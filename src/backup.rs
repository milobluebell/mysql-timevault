use crate::{binlog, config::Config, db, Result};
use chrono::{DateTime, Utc};
use flate2::{write::GzEncoder, Compression};
use mysql::prelude::Queryable;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub databases: Vec<String>,
    pub metadata_schema: String,
    pub source_server_uuid: String,
    pub binlog_file: String,
    pub binlog_position: u64,
    pub bytes: u64,
    pub sha256: String,
    pub pitr_scope_complete: bool,
}

fn backup_dir(root: &Path) -> PathBuf {
    root.join("backups")
}

fn valid_backup_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn list(root: &Path) -> Result<Vec<Manifest>> {
    let mut result = Vec::new();
    let dir = backup_dir(root);
    if !dir.exists() {
        return Ok(result);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path().join("manifest.json");
        if path.is_file() {
            let manifest: Manifest = serde_json::from_slice(&fs::read(&path)?)?;
            let name = entry.file_name();
            let name = name.to_str().ok_or("backup directory name is not UTF-8")?;
            if !valid_backup_id(name) || manifest.id != name {
                return Err(format!("invalid backup manifest ID in {}", path.display()).into());
            }
            result.push(manifest);
        }
    }
    result.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    Ok(result)
}

pub fn get(root: &Path, id: &str) -> Result<Manifest> {
    if !valid_backup_id(id) {
        return Err("invalid backup id".into());
    }
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(backup_dir(root).join(id).join("manifest.json"))?)?;
    if manifest.id != id {
        return Err("backup manifest id does not match directory".into());
    }
    Ok(manifest)
}

pub fn oldest_binlog(root: &Path) -> Result<Option<String>> {
    Ok(list(root)?.first().map(|m| m.binlog_file.clone()))
}

pub fn remove_stale_partials(root: &Path) -> Result<()> {
    let dir = backup_dir(root);
    crate::private_directory(&dir)?;
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && entry.file_name().to_string_lossy().starts_with(".partial-")
        {
            fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

pub fn run(
    config: &Config,
    password: &str,
    root: &Path,
    metadata_schema: &str,
) -> Result<Manifest> {
    crate::private_directory(&backup_dir(root))?;
    let suffix = Uuid::new_v4().simple().to_string();
    let id = format!("{}-{}", Utc::now().format("%Y%m%dT%H%M%SZ"), &suffix[..8]);
    let created_at = Utc::now();
    let mut conn = db::connect(config, password)?;
    let source_server_uuid: String = conn
        .query_first("SELECT @@server_uuid")?
        .ok_or("source has no server UUID")?;
    let source_version: String = conn
        .query_first("SELECT VERSION()")?
        .ok_or("source has no version")?;
    let dump_client = dump_client(&source_version)?;
    db::start_run(&mut conn, metadata_schema, &id, &config.backup.databases)?;
    drop(conn);

    let staging = backup_dir(root).join(format!(".partial-{id}"));
    let published = backup_dir(root).join(&id);
    fs::create_dir(&staging)?;
    let outcome = (|| -> Result<Manifest> {
        let (file, position) = dump(
            config,
            password,
            metadata_schema,
            dump_client,
            &staging.join("snapshot.sql.gz"),
        )?;
        let copied = binlog::sync(config, password, root, &file)?;
        let first = binlog::local_from(root, &file)?
            .into_iter()
            .next()
            .ok_or("source binary log was not copied")?;
        if fs::metadata(first)?.len() < position {
            return Err("source binary log is shorter than snapshot coordinate".into());
        }
        println!("binary logs synchronized: {copied} files");
        let snapshot = staging.join("snapshot.sql.gz");
        let (bytes, sha256) = hash_file(&snapshot)?;
        let complete = pitr_scope_complete(config, password, metadata_schema)?;
        let manifest = Manifest {
            id: id.clone(),
            created_at,
            completed_at: Utc::now(),
            databases: config.backup.databases.clone(),
            metadata_schema: metadata_schema.to_owned(),
            source_server_uuid,
            binlog_file: file,
            binlog_position: position,
            bytes,
            sha256,
            pitr_scope_complete: complete,
        };
        let manifest_path = staging.join("manifest.json");
        let mut file = File::create(&manifest_path)?;
        serde_json::to_writer_pretty(&mut file, &manifest)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&staging, &published)?;
        let mut conn = db::connect(config, password)?;
        db::complete_run(
            &mut conn,
            metadata_schema,
            &manifest,
            &published.to_string_lossy(),
        )?;
        if let Err(err) = prune(config, password, root, metadata_schema) {
            eprintln!("backup succeeded but retention cleanup failed: {err}");
        }
        Ok(manifest)
    })();

    if let Err(err) = &outcome {
        if staging.exists() {
            let _ = fs::remove_dir_all(&staging);
        }
        if !published.exists() {
            if let Ok(mut conn) = db::connect(config, password) {
                let _ = db::fail_run(&mut conn, metadata_schema, &id, &err.to_string());
            }
        }
    }
    outcome
}

fn pitr_scope_complete(config: &Config, password: &str, metadata_schema: &str) -> Result<bool> {
    let mut conn = db::connect(config, password)?;
    let names: Vec<String> = conn.query("SELECT SCHEMA_NAME FROM information_schema.SCHEMATA")?;
    let present: HashSet<_> = names
        .into_iter()
        .filter(|name| {
            !matches!(
                name.as_str(),
                "mysql" | "sys" | "information_schema" | "performance_schema"
            ) && name != metadata_schema
        })
        .collect();
    let configured: HashSet<_> = config.backup.databases.iter().cloned().collect();
    Ok(present == configured)
}

fn dump(
    config: &Config,
    password: &str,
    metadata_schema: &str,
    dump_client: &str,
    target: &Path,
) -> Result<(String, u64)> {
    let mut command = Command::new(dump_client);
    command
        .arg("--single-transaction")
        .arg("--quick")
        .arg("--master-data=2")
        .arg("--routines")
        .arg("--events")
        .arg("--triggers")
        .arg("--no-tablespaces")
        .arg("--skip-column-statistics")
        .arg("--set-gtid-purged=OFF")
        .arg("--default-character-set=utf8mb4")
        .arg("--hex-blob")
        .arg(format!("--host={}", config.mysql.host))
        .arg(format!("--port={}", config.mysql.port))
        .arg(format!("--user={}", config.mysql.user))
        .arg("--protocol=tcp")
        .arg("--skip-lock-tables")
        .arg("--databases")
        .args(&config.backup.databases)
        .arg(metadata_schema)
        .env("MYSQL_PWD", password)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take().ok_or("mysqldump stdout unavailable")?;
    let result = (|| -> Result<(String, u64)> {
        let mut prefix = Vec::new();
        let mut chunk = [0u8; 8192];
        let position = loop {
            if let Some(position) = parse_source_coordinates(&prefix) {
                break position;
            }
            if prefix.len() >= 2 * 1024 * 1024 {
                return Err(
                    "mysqldump did not emit binary log coordinates in its first 2 MiB".into(),
                );
            }
            let n = stdout.read(&mut chunk)?;
            if n == 0 {
                return Err("mysqldump ended without binary log coordinates".into());
            }
            prefix.extend_from_slice(&chunk[..n]);
        };
        let file = File::create(target)?;
        let mut encoder = GzEncoder::new(file, Compression::default());
        encoder.write_all(&prefix)?;
        io::copy(&mut stdout, &mut encoder)?;
        let file = encoder.finish()?;
        file.sync_all()?;
        let status = child.wait()?;
        if !status.success() {
            return Err(format!("mysqldump failed: {status}").into());
        }
        Ok(position)
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn dump_client(version: &str) -> Result<&'static str> {
    let mut parts = version.split('.');
    let major: u32 = parts.next().ok_or("invalid MySQL version")?.parse()?;
    let minor: u32 = parts.next().ok_or("invalid MySQL version")?.parse()?;
    match (major, minor) {
        (5, 7) | (8, 0) => Ok("mysqldump80"),
        (8, 4) => Ok("mysqldump"),
        _ => Err(format!("unsupported MySQL version: {version}").into()),
    }
}

fn parse_source_coordinates(bytes: &[u8]) -> Option<(String, u64)> {
    let text = String::from_utf8_lossy(bytes);
    for line in text.lines() {
        if !line.contains("CHANGE REPLICATION SOURCE TO") && !line.contains("CHANGE MASTER TO") {
            continue;
        }
        let (file_key, pos_key) = if line.contains("SOURCE_LOG_FILE='") {
            ("SOURCE_LOG_FILE='", "SOURCE_LOG_POS=")
        } else {
            ("MASTER_LOG_FILE='", "MASTER_LOG_POS=")
        };
        let file_start = line.find(file_key)? + file_key.len();
        let rest = &line[file_start..];
        let file_end = rest.find('\'')?;
        let file = rest[..file_end].to_owned();
        let pos_start = line.find(pos_key)? + pos_key.len();
        let pos: String = line[pos_start..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if !file.is_empty() {
            return Some((file, pos.parse().ok()?));
        }
    }
    None
}

fn hash_file(path: &Path) -> Result<(u64, String)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut bytes = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        bytes += n as u64;
    }
    let hash = hasher.finalize();
    Ok((bytes, format!("{hash:x}")))
}

pub fn verify_snapshot(root: &Path, manifest: &Manifest) -> Result<PathBuf> {
    let path = backup_dir(root).join(&manifest.id).join("snapshot.sql.gz");
    let (bytes, hash) = hash_file(&path)?;
    if bytes != manifest.bytes || hash != manifest.sha256 {
        return Err(format!("backup {} failed checksum verification", manifest.id).into());
    }
    Ok(path)
}

fn prune(config: &Config, password: &str, root: &Path, schema: &str) -> Result<()> {
    let manifests = list(root)?;
    let excess = manifests.len().saturating_sub(config.backup.max_backups);
    for manifest in manifests.iter().take(excess) {
        let mut conn = db::connect(config, password)?;
        db::prune_run(&mut conn, schema, &manifest.id)?;
        fs::remove_dir_all(backup_dir(root).join(&manifest.id))?;
        println!("old backup removed: {}", manifest.id);
    }
    if let Some(oldest) = oldest_binlog(root)? {
        binlog::prune_before(root, &oldest)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commented_source_coordinates() {
        let text = b"-- header\n-- CHANGE REPLICATION SOURCE TO SOURCE_LOG_FILE='binlog.000042', SOURCE_LOG_POS=1319;\n";
        assert_eq!(
            parse_source_coordinates(text),
            Some(("binlog.000042".into(), 1319))
        );
    }

    #[test]
    fn parses_mysql_57_master_coordinates() {
        let text = b"-- CHANGE MASTER TO MASTER_LOG_FILE='mysql-bin.000001', MASTER_LOG_POS=154;\n";
        assert_eq!(
            parse_source_coordinates(text),
            Some(("mysql-bin.000001".into(), 154))
        );
    }

    #[test]
    fn chooses_dump_client_for_supported_servers() {
        assert_eq!(dump_client("5.7.44").unwrap(), "mysqldump80");
        assert_eq!(dump_client("8.0.46").unwrap(), "mysqldump80");
        assert_eq!(dump_client("8.4.11").unwrap(), "mysqldump");
        assert!(dump_client("8.1.0").is_err());
    }

    #[test]
    fn rejects_modified_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("backups/version-1");
        fs::create_dir_all(&dir).unwrap();
        let snapshot = dir.join("snapshot.sql.gz");
        fs::write(&snapshot, b"original").unwrap();
        let (bytes, sha256) = hash_file(&snapshot).unwrap();
        let manifest = Manifest {
            id: "version-1".into(),
            created_at: Utc::now(),
            completed_at: Utc::now(),
            databases: vec!["app".into()],
            metadata_schema: "mm_0123456789ab".into(),
            source_server_uuid: "source".into(),
            binlog_file: "binlog.000001".into(),
            binlog_position: 4,
            bytes,
            sha256,
            pitr_scope_complete: true,
        };
        assert!(verify_snapshot(root.path(), &manifest).is_ok());
        fs::write(snapshot, b"modified").unwrap();
        assert!(verify_snapshot(root.path(), &manifest).is_err());
    }

    #[test]
    fn rejects_manifest_id_outside_backup_directory() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("backups/version-1");
        fs::create_dir_all(&dir).unwrap();
        let manifest = Manifest {
            id: "../../unrelated".into(),
            created_at: Utc::now(),
            completed_at: Utc::now(),
            databases: vec!["app".into()],
            metadata_schema: "mm_0123456789ab".into(),
            source_server_uuid: "source".into(),
            binlog_file: "binlog.000001".into(),
            binlog_position: 4,
            bytes: 0,
            sha256: String::new(),
            pitr_scope_complete: false,
        };
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(list(root.path()).is_err());
    }
}
