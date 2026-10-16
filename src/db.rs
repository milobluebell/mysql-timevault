use crate::{
    backup,
    config::{Config, Credentials},
    Result,
};
use chrono::{NaiveDateTime, Utc};
use mysql::{params, prelude::Queryable, Conn, OptsBuilder};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::Path,
};

pub fn connect(config: &Config, credentials: &Credentials) -> Result<Conn> {
    let opts = OptsBuilder::new()
        .ip_or_hostname(Some(config.mysql.host.clone()))
        .tcp_port(config.mysql.port)
        .user(Some(credentials.user.clone()))
        .pass(Some(credentials.password.clone()));
    let mut conn = Conn::new(opts)?;
    conn.query_drop("SET SESSION time_zone = '+00:00'")?;
    Ok(conn)
}

pub fn metadata_schema(conn: &mut Conn, root: &Path) -> Result<String> {
    let marker = root.join(".metadata-schema");
    let manifests = backup::list(root)?;
    let existing_identity = marker.exists() || !manifests.is_empty();
    let schema = if marker.exists() {
        fs::read_to_string(&marker)?.trim().to_owned()
    } else if let Some(manifest) = manifests.first() {
        manifest.metadata_schema.clone()
    } else {
        format!("mm_{}", &uuid::Uuid::new_v4().simple().to_string()[..12])
    };
    if schema.len() != 15
        || !schema.starts_with("mm_")
        || !schema[3..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("invalid metadata schema name".into());
    }
    let exists: Option<String> = conn.exec_first(
        "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
        (&schema,),
    )?;
    if exists.is_none() && existing_identity {
        return Err(format!(
            "metadata database {schema} is missing on the configured MySQL server"
        )
        .into());
    }
    if exists.is_none() {
        conn.query_drop(format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4"))?;
    }
    if !marker.exists() {
        fs::write(marker, &schema)?;
    }
    Ok(schema)
}

pub fn setup(conn: &mut Conn, schema: &str) -> Result<()> {
    let q = |table: &str| format!("`{schema}`.`{table}`");
    conn.query_drop(format!(
        "CREATE TABLE IF NOT EXISTS {} (
        id VARCHAR(64) PRIMARY KEY,
        started_at DATETIME(6) NOT NULL,
        finished_at DATETIME(6) NULL,
        status VARCHAR(16) NOT NULL,
        databases_json JSON NOT NULL,
        manifest_path VARCHAR(512) NULL,
        bytes BIGINT UNSIGNED NULL,
        sha256 CHAR(64) NULL,
        source_server_uuid CHAR(36) NULL,
        binlog_file VARCHAR(255) NULL,
        binlog_position BIGINT UNSIGNED NULL,
        pitr_scope_complete BOOLEAN NULL,
        error_text TEXT NULL,
        INDEX (started_at)
    ) ENGINE=InnoDB",
        q("backup_runs")
    ))?;
    conn.query_drop(format!(
        "CREATE TABLE IF NOT EXISTS {} (
        event_hash BINARY(32) PRIMARY KEY,
        logged_at DATETIME(6) NOT NULL,
        thread_id BIGINT UNSIGNED NOT NULL,
        priority VARCHAR(32) NOT NULL,
        error_code VARCHAR(64) NOT NULL,
        subsystem VARCHAR(128) NOT NULL,
        message LONGBLOB NOT NULL,
        collected_at DATETIME(6) NOT NULL,
        INDEX (logged_at)
    ) ENGINE=InnoDB",
        q("error_events")
    ))?;
    conn.query_drop(format!(
        "CREATE TABLE IF NOT EXISTS {} (
        event_hash BINARY(32) PRIMARY KEY,
        started_at DATETIME(6) NOT NULL,
        thread_id BIGINT UNSIGNED NOT NULL,
        user_host TEXT NOT NULL,
        database_name VARCHAR(512) NULL,
        query_time_us BIGINT UNSIGNED NOT NULL,
        lock_time_us BIGINT UNSIGNED NOT NULL,
        rows_sent BIGINT UNSIGNED NOT NULL,
        rows_examined BIGINT UNSIGNED NOT NULL,
        sql_text LONGBLOB NOT NULL,
        collected_at DATETIME(6) NOT NULL,
        INDEX (started_at)
    ) ENGINE=InnoDB",
        q("slow_queries")
    ))?;
    conn.query_drop(format!(
        "CREATE TABLE IF NOT EXISTS {} (
        source VARCHAR(16) PRIMARY KEY,
        cursor_time DATETIME(6) NOT NULL,
        cursor_thread BIGINT UNSIGNED NOT NULL
    ) ENGINE=InnoDB",
        q("log_cursors")
    ))?;
    conn.exec_drop(
        format!(
            "INSERT IGNORE INTO {} (source, cursor_time, cursor_thread)
        VALUES ('slow', '1970-01-01 00:00:00.000000', 0)",
            q("log_cursors")
        ),
        (),
    )?;
    conn.query_drop(format!(
        "INSERT IGNORE INTO {} (source, cursor_time, cursor_thread)
         VALUES ('error', '1970-01-01 00:00:00.000000', 0)",
        q("log_cursors")
    ))?;
    Ok(())
}

fn now() -> String {
    Utc::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string()
}

pub fn start_run(conn: &mut Conn, schema: &str, id: &str, databases: &[String]) -> Result<()> {
    let sql = format!(
        "INSERT INTO `{schema}`.`backup_runs`
        (id, started_at, status, databases_json) VALUES (:id, :started, 'running', :databases)"
    );
    conn.exec_drop(
        sql,
        params! {
            "id" => id,
            "started" => now(),
            "databases" => serde_json::to_string(databases)?,
        },
    )?;
    Ok(())
}

pub fn complete_run(
    conn: &mut Conn,
    schema: &str,
    manifest: &crate::backup::Manifest,
    path: &str,
) -> Result<()> {
    let sql = format!(
        "UPDATE `{schema}`.`backup_runs` SET
        finished_at=:finished, status='succeeded', manifest_path=:path,
        bytes=:bytes, sha256=:sha, binlog_file=:binlog_file,
        binlog_position=:binlog_position, source_server_uuid=:source_uuid,
        pitr_scope_complete=:pitr_complete, error_text=NULL WHERE id=:id"
    );
    conn.exec_drop(
        sql,
        params! {
            "finished" => manifest.completed_at.format("%Y-%m-%d %H:%M:%S%.6f").to_string(),
            "path" => path,
            "bytes" => manifest.bytes,
            "sha" => &manifest.sha256,
            "binlog_file" => &manifest.binlog_file,
            "binlog_position" => manifest.binlog_position,
            "source_uuid" => &manifest.source_server_uuid,
            "pitr_complete" => manifest.pitr_scope_complete,
            "id" => &manifest.id,
        },
    )?;
    Ok(())
}

pub fn fail_run(conn: &mut Conn, schema: &str, id: &str, error: &str) -> Result<()> {
    let sql = format!(
        "UPDATE `{schema}`.`backup_runs` SET finished_at=:finished,
        status='failed', error_text=:error WHERE id=:id"
    );
    conn.exec_drop(
        sql,
        params! {
            "finished" => now(),
            "error" => error.chars().take(4000).collect::<String>(),
            "id" => id,
        },
    )?;
    Ok(())
}

pub fn prune_run(conn: &mut Conn, schema: &str, id: &str) -> Result<()> {
    conn.exec_drop(
        format!("UPDATE `{schema}`.`backup_runs` SET status='pruned' WHERE id=?"),
        (id,),
    )?;
    Ok(())
}

pub fn reconcile_runs(conn: &mut Conn, schema: &str, root: &std::path::Path) -> Result<()> {
    let manifests = backup::list(root)?;
    let available: std::collections::HashSet<_> = manifests.iter().map(|m| m.id.as_str()).collect();
    for manifest in &manifests {
        let status: Option<String> = conn.exec_first(
            format!("SELECT status FROM `{schema}`.`backup_runs` WHERE id=?"),
            (&manifest.id,),
        )?;
        if status.is_none() {
            start_run(conn, schema, &manifest.id, &manifest.databases)?;
        }
        if status.as_deref() != Some("succeeded") {
            let path = root
                .join("backups")
                .join(&manifest.id)
                .to_string_lossy()
                .to_string();
            complete_run(conn, schema, manifest, &path)?;
        }
    }
    let rows: Vec<(String, String)> = conn.query(format!(
        "SELECT id, status FROM `{schema}`.`backup_runs` WHERE status IN ('running','succeeded')"
    ))?;
    for (id, status) in rows {
        if !available.contains(id.as_str()) {
            if status == "running" {
                fail_run(conn, schema, &id, "snapshot missing after interrupted run")?;
            } else {
                conn.exec_drop(
                    format!("UPDATE `{schema}`.`backup_runs` SET status='missing' WHERE id=?"),
                    (&id,),
                )?;
            }
        }
    }
    Ok(())
}

pub fn ingest_logs(
    conn: &mut Conn,
    schema: &str,
    error_log_file: Option<&Path>,
) -> Result<(usize, usize)> {
    let errors = if let Some(path) = error_log_file {
        ingest_errors(conn, schema, path)?
    } else {
        0
    };
    let slow = ingest_slow(conn, schema)?;
    Ok((errors, slow))
}

fn hash_fields(parts: &[&[u8]]) -> Vec<u8> {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part);
    }
    hash.finalize().to_vec()
}

fn ingest_errors(conn: &mut Conn, schema: &str, path: &Path) -> Result<usize> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut offset: u64 = conn
        .query_first(format!(
            "SELECT cursor_thread FROM `{schema}`.`log_cursors` WHERE source='error'"
        ))?
        .ok_or("missing error log cursor")?;
    if reader.get_ref().metadata()?.len() < offset {
        offset = 0;
    }
    reader.seek(SeekFrom::Start(offset))?;
    let mut added = 0;
    let sql = format!(
        "INSERT IGNORE INTO `{schema}`.`error_events`
        (event_hash, logged_at, thread_id, priority, error_code, subsystem, message, collected_at)
        VALUES (:hash, :logged, :thread, :priority, :code, :subsystem, :message, :collected)"
    );
    loop {
        let mut message = Vec::new();
        let read = reader.read_until(b'\n', &mut message)?;
        if read == 0 || !message.ends_with(b"\n") {
            break;
        }
        let logged = error_log_timestamp(&message).unwrap_or_else(now);
        let hash = hash_fields(&[&offset.to_be_bytes(), &message]);
        conn.exec_drop(
            &sql,
            params! {
                "hash" => hash,
                "logged" => &logged,
                "thread" => 0_u64,
                "priority" => "FILE",
                "code" => "",
                "subsystem" => "mysqld",
                "message" => &message,
                "collected" => now(),
            },
        )?;
        added += conn.affected_rows() as usize;
        offset += read as u64;
    }
    conn.exec_drop(
        format!(
        "UPDATE `{schema}`.`log_cursors` SET cursor_time=?, cursor_thread=? WHERE source='error'"
    ),
        (now(), offset),
    )?;
    Ok(added)
}

fn error_log_timestamp(line: &[u8]) -> Option<String> {
    let head = std::str::from_utf8(line.get(..19)?).ok()?;
    let normalized = head.replace('T', " ");
    NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|time| time.format("%Y-%m-%d %H:%M:%S%.6f").to_string())
}

fn ingest_slow(conn: &mut Conn, schema: &str) -> Result<usize> {
    type SlowRow = (
        String,
        u64,
        String,
        Option<String>,
        u64,
        u64,
        u64,
        u64,
        Vec<u8>,
    );
    let cursor_sql = format!(
        "SELECT DATE_FORMAT(cursor_time, '%Y-%m-%d %H:%i:%s.%f'), cursor_thread
        FROM `{schema}`.`log_cursors` WHERE source='slow'"
    );
    let (cursor_time, _): (String, u64) = conn
        .query_first(cursor_sql)?
        .ok_or("missing slow log cursor")?;
    let cutoff: String = conn
        .query_first("SELECT DATE_FORMAT(UTC_TIMESTAMP(6), '%Y-%m-%d %H:%i:%s.%f')")?
        .ok_or("could not read server time")?;
    let query = "SELECT DATE_FORMAT(start_time, '%Y-%m-%d %H:%i:%s.%f'),
        thread_id, user_host, db,
        CAST(TIME_TO_SEC(query_time) * 1000000 AS UNSIGNED),
        CAST(TIME_TO_SEC(lock_time) * 1000000 AS UNSIGNED),
        rows_sent, rows_examined, sql_text
        FROM mysql.slow_log
        WHERE start_time >= DATE_SUB(?, INTERVAL 1 DAY) AND start_time <= ?
        ORDER BY start_time, thread_id LIMIT 500 OFFSET ?";
    let mut total = 0;
    let mut offset = 0;
    loop {
        let rows: Vec<SlowRow> = conn.exec(query, (&cursor_time, &cutoff, offset))?;
        if rows.is_empty() {
            break;
        }
        let count = rows.len();
        let insert = format!(
            "INSERT IGNORE INTO `{schema}`.`slow_queries`
            (event_hash, started_at, thread_id, user_host, database_name,
             query_time_us, lock_time_us, rows_sent, rows_examined, sql_text, collected_at)
            VALUES (:hash, :started, :thread, :user_host, :db, :query_us,
                :lock_us, :sent, :examined, :sql, :collected)"
        );
        for (
            started,
            thread_id,
            user_host,
            database,
            query_us,
            lock_us,
            sent,
            examined,
            sql_text,
        ) in rows
        {
            let thread = thread_id.to_be_bytes();
            let hash = hash_fields(&[started.as_bytes(), &thread, user_host.as_bytes(), &sql_text]);
            conn.exec_drop(
                &insert,
                params! {
                    "hash" => hash,
                    "started" => &started,
                    "thread" => thread_id,
                    "user_host" => &user_host,
                    "db" => &database,
                    "query_us" => query_us,
                    "lock_us" => lock_us,
                    "sent" => sent,
                    "examined" => examined,
                    "sql" => &sql_text,
                    "collected" => now(),
                },
            )?;
            total += conn.affected_rows() as usize;
        }
        offset += count;
        if count < 500 {
            break;
        }
    }
    conn.exec_drop(
        format!(
            "UPDATE `{schema}`.`log_cursors`
        SET cursor_time=?, cursor_thread=0 WHERE source='slow'"
        ),
        (&cutoff,),
    )?;
    Ok(total)
}
