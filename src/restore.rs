use crate::{backup, binlog, db, Result};
use chrono::{DateTime, Utc};
use flate2::read::GzDecoder;
use mysql::{prelude::Queryable, Conn, OptsBuilder};
use std::{
    env,
    fs::File,
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

struct Target {
    host: String,
    port: u16,
    user: String,
    password: String,
}

pub fn run(root: &Path, args: &[String]) -> Result<()> {
    let id = args.first().ok_or("restore needs a backup ID")?;
    let mut host = None;
    let mut port = 3306;
    let mut user = None;
    let mut password_env = None;
    let mut at = None;
    let mut index = 1;
    while index < args.len() {
        let flag = &args[index];
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--target-host" => host = Some(value.clone()),
            "--target-port" => port = value.parse()?,
            "--target-user" => user = Some(value.clone()),
            "--target-password-env" => password_env = Some(value.clone()),
            "--at" => {
                let time = DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc);
                if time.timestamp_subsec_nanos() != 0 {
                    return Err("--at supports whole-second timestamps only".into());
                }
                at = Some(time);
            }
            _ => return Err(format!("unknown restore option: {flag}").into()),
        }
        index += 2;
    }
    let host = host.ok_or("--target-host is required")?;
    let user = user.ok_or("--target-user is required")?;
    let password_env = password_env.ok_or("--target-password-env is required")?;
    if host.trim().is_empty() || user.trim().is_empty() || port == 0 {
        return Err("target host, user, and port must be valid".into());
    }
    let password = env::var(&password_env).map_err(|_| format!("missing {password_env}"))?;
    if password.is_empty() {
        return Err("target password is empty".into());
    }
    let target = Target {
        host,
        port,
        user,
        password,
    };

    let manifest = backup::get(root, id)?;
    let snapshot = backup::verify_snapshot(root, &manifest)?;
    let replay_files = if let Some(at) = at {
        if !manifest.pitr_scope_complete {
            return Err(
                "this snapshot omits a user database; point-in-time recovery is unavailable".into(),
            );
        }
        if at < manifest.completed_at {
            return Err("recovery time precedes completion of the selected snapshot".into());
        }
        let coverage = binlog::read_coverage(root)?;
        if at > coverage.covered_through {
            return Err(format!(
                "binary logs cover only through {}",
                coverage.covered_through
            )
            .into());
        }
        let files = binlog::local_from(root, &manifest.binlog_file)?;
        if files
            .first()
            .map(std::fs::metadata)
            .transpose()?
            .map(|m| m.len())
            .unwrap_or_default()
            < manifest.binlog_position
        {
            return Err("source binary log is shorter than snapshot coordinate".into());
        }
        let last = files
            .last()
            .and_then(|p| p.file_name())
            .and_then(|p| p.to_str());
        if last != Some(coverage.last_file.as_str()) {
            return Err("binary log coverage does not match local files".into());
        }
        run_binlog(&files, manifest.binlog_position, at, None)?;
        files
    } else {
        Vec::new()
    };

    let opts = OptsBuilder::new()
        .ip_or_hostname(Some(target.host.clone()))
        .tcp_port(target.port)
        .user(Some(target.user.clone()))
        .pass(Some(target.password.clone()));
    let mut conn = Conn::new(opts)?;
    let uuid: String = conn
        .query_first("SELECT @@server_uuid")?
        .ok_or("target has no server UUID")?;
    if uuid == manifest.source_server_uuid {
        return Err(
            "target is the backup source MySQL server; use an independent empty target".into(),
        );
    }
    let names: Vec<String> = conn.query("SELECT SCHEMA_NAME FROM information_schema.SCHEMATA")?;
    let non_system: Vec<_> = names
        .into_iter()
        .filter(|s| {
            !matches!(
                s.as_str(),
                "mysql" | "sys" | "information_schema" | "performance_schema"
            )
        })
        .collect();
    if !non_system.is_empty() {
        return Err(format!(
            "target has user databases ({}) and must be empty",
            non_system.join(", ")
        )
        .into());
    }
    drop(conn);

    println!(
        "restoring backup {} to {}:{}",
        manifest.id, target.host, target.port
    );
    let mut mysql = mysql_client(&target)?;
    let result = (|| -> Result<()> {
        let input = mysql.stdin.as_mut().ok_or("mysql stdin unavailable")?;
        let mut decoder = GzDecoder::new(File::open(snapshot)?);
        io::copy(&mut decoder, input)?;
        if let Some(at) = at {
            run_binlog(
                &replay_files,
                manifest.binlog_position,
                at,
                mysql.stdin.as_mut(),
            )?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = mysql.kill();
    }
    drop(mysql.stdin.take());
    let status = mysql.wait()?;
    result?;
    if !status.success() {
        return Err(format!("mysql restore client failed: {status}").into());
    }
    if at.is_none() {
        let opts = OptsBuilder::new()
            .ip_or_hostname(Some(target.host.clone()))
            .tcp_port(target.port)
            .user(Some(target.user.clone()))
            .pass(Some(target.password.clone()));
        let mut conn = Conn::new(opts)?;
        db::complete_run(
            &mut conn,
            &manifest.metadata_schema,
            &manifest,
            &root.join("backups").join(&manifest.id).to_string_lossy(),
        )?;
    }
    println!(
        "restore completed: {}{}",
        manifest.id,
        at.map(|x| format!(" through {x}")).unwrap_or_default()
    );
    Ok(())
}

fn mysql_client(target: &Target) -> Result<std::process::Child> {
    Ok(Command::new("mysql")
        .arg("--binary-mode")
        .arg("--protocol=tcp")
        .arg(format!("--host={}", target.host))
        .arg(format!("--port={}", target.port))
        .arg(format!("--user={}", target.user))
        .env("MYSQL_PWD", &target.password)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?)
}

fn run_binlog(
    files: &[PathBuf],
    start: u64,
    stop: DateTime<Utc>,
    mut output: Option<&mut std::process::ChildStdin>,
) -> Result<()> {
    let mut command = Command::new("mysqlbinlog");
    command
        .arg("--verify-binlog-checksum")
        .arg(format!("--start-position={start}"))
        .arg(format!(
            "--stop-datetime={}",
            stop.format("%Y-%m-%d %H:%M:%S")
        ))
        .env("TZ", "UTC");
    command
        .args(files)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = command.spawn()?;
    let result = (|| -> Result<()> {
        let mut stdout = child
            .stdout
            .take()
            .ok_or("mysqlbinlog stdout unavailable")?;
        if let Some(ref mut writer) = output {
            io::copy(&mut stdout, *writer)?;
        } else {
            io::copy(&mut stdout, &mut io::sink())?;
        }
        let status = child.wait()?;
        if !status.success() {
            return Err(format!("mysqlbinlog failed: {status}").into());
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}
