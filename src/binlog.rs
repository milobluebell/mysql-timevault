use crate::{config::Config, db, Result};
use chrono::{DateTime, Utc};
use mysql::{prelude::Queryable, Conn, Row};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use uuid::Uuid;

fn valid_file(name: &str) -> bool {
    let Some((prefix, suffix)) = name.rsplit_once('.') else {
        return false;
    };
    !prefix.is_empty()
        && !prefix.starts_with('-')
        && !suffix.is_empty()
        && prefix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
        && suffix.bytes().all(|b| b.is_ascii_digit())
}

pub fn remote_logs(conn: &mut Conn) -> Result<Vec<(String, u64)>> {
    let rows: Vec<Row> = conn.query("SHOW BINARY LOGS")?;
    let mut logs = Vec::new();
    for row in rows {
        let name: String = row.get(0).ok_or("SHOW BINARY LOGS did not return a name")?;
        let size: u64 = row.get(1).ok_or("SHOW BINARY LOGS did not return a size")?;
        if !valid_file(&name) {
            return Err(format!("unsafe binary log name: {name}").into());
        }
        logs.push((name, size));
    }
    if logs.is_empty() {
        return Err("binary logging is disabled or SHOW BINARY LOGS returned no files".into());
    }
    Ok(logs)
}

pub fn sync(config: &Config, password: &str, root: &Path, from_file: &str) -> Result<usize> {
    if !valid_file(from_file) {
        return Err("invalid source binary log file name".into());
    }
    let mut conn = db::connect(config, password)?;
    let logs = remote_logs(&mut conn)?;
    let covered_through = Utc::now();
    let start = logs
        .iter()
        .position(|(name, _)| name == from_file)
        .ok_or_else(|| format!("required binary log {from_file} is no longer on the server"))?;
    let dir = root.join("binlogs");
    crate::private_directory(&dir)?;
    let mut copied = 0;
    for (name, remote_size) in &logs[start..] {
        let target = dir.join(name);
        if fs::metadata(&target).is_ok_and(|meta| meta.len() >= *remote_size) {
            continue;
        }
        let staging = dir.join(format!(".partial-{}", Uuid::new_v4().simple()));
        fs::create_dir(&staging)?;
        let result = (|| -> Result<()> {
            let status = Command::new("mysqlbinlog")
                .arg("--read-from-remote-server")
                .arg("--raw")
                .arg(format!("--host={}", config.mysql.host))
                .arg(format!("--port={}", config.mysql.port))
                .arg(format!("--user={}", config.mysql.user))
                .arg("--protocol=tcp")
                .arg(name)
                .env("MYSQL_PWD", password)
                .current_dir(&staging)
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .status()?;
            if !status.success() {
                return Err(format!("mysqlbinlog failed for {name}: {status}").into());
            }
            let downloaded = staging.join(name);
            let file = fs::File::open(&downloaded)?;
            if file.metadata()?.len() < *remote_size {
                return Err(format!("incomplete binary log {name}").into());
            }
            file.sync_all()?;
            fs::rename(downloaded, &target)?;
            copied += 1;
            Ok(())
        })();
        let _ = fs::remove_dir_all(&staging);
        result?;
    }
    write_coverage(
        root,
        &Coverage {
            covered_through,
            last_file: logs.last().unwrap().0.clone(),
        },
    )?;
    Ok(copied)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Coverage {
    pub covered_through: DateTime<Utc>,
    pub last_file: String,
}

fn write_coverage(root: &Path, coverage: &Coverage) -> Result<()> {
    let dir = root.join("binlogs");
    let temp = dir.join(format!(".coverage-{}.tmp", Uuid::new_v4().simple()));
    fs::write(&temp, serde_json::to_vec_pretty(coverage)?)?;
    fs::rename(temp, dir.join("coverage.json"))?;
    Ok(())
}

pub fn read_coverage(root: &Path) -> Result<Coverage> {
    Ok(serde_json::from_slice(&fs::read(
        root.join("binlogs/coverage.json"),
    )?)?)
}

pub fn local_from(root: &Path, start: &str) -> Result<Vec<PathBuf>> {
    if !valid_file(start) {
        return Err("invalid source binary log name".into());
    }
    let prefix = start.rsplit_once('.').unwrap().0;
    let dir = root.join("binlogs");
    let mut files = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if entry.file_type()?.is_file()
            && valid_file(&name)
            && name.rsplit_once('.').is_some_and(|(p, _)| p == prefix)
            && name.as_str() >= start
        {
            files.push(entry.path());
        }
    }
    files.sort();
    if files
        .first()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        != Some(start)
    {
        return Err(format!("binary log {start} is missing from {}", dir.display()).into());
    }
    verify_contiguous(&files)?;
    Ok(files)
}

fn verify_contiguous(files: &[PathBuf]) -> Result<()> {
    for pair in files.windows(2) {
        let a = pair[0]
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("invalid binlog file name")?;
        let b = pair[1]
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("invalid binlog file name")?;
        let (prefix_a, number_a) = a
            .rsplit_once('.')
            .ok_or("binary log has no numeric suffix")?;
        let (prefix_b, number_b) = b
            .rsplit_once('.')
            .ok_or("binary log has no numeric suffix")?;
        if prefix_a != prefix_b || number_b.parse::<u64>()? != number_a.parse::<u64>()? + 1 {
            return Err(format!("binary log gap between {a} and {b}").into());
        }
    }
    Ok(())
}

pub fn prune_before(root: &Path, oldest_needed: &str) -> Result<()> {
    let dir = root.join("binlogs");
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if entry.file_type()?.is_file() && valid_file(&name) && name.as_str() < oldest_needed {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_missing_binlog_segment() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("binlogs")).unwrap();
        fs::write(root.path().join("binlogs/mysql-bin.000001"), b"a").unwrap();
        fs::write(root.path().join("binlogs/mysql-bin.000003"), b"b").unwrap();
        assert!(local_from(root.path(), "mysql-bin.000001").is_err());
    }

    #[test]
    fn ignores_coverage_metadata_when_listing_logs() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("binlogs");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("mysql-bin.000001"), b"a").unwrap();
        fs::write(dir.join("coverage.json"), b"{}").unwrap();
        assert_eq!(
            local_from(root.path(), "mysql-bin.000001").unwrap().len(),
            1
        );
    }
}
