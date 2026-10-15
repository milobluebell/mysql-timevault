use chrono::Utc;
use mysql::{prelude::Queryable, Conn};
use mysql_timevault::{backup::Manifest, binlog, config::Config, db, Result};
use std::{env, fs};
use uuid::Uuid;

fn connect() -> Result<Conn> {
    let port: u16 = env::var("MYSQL_TEST_PORT")?.parse()?;
    let password = env::var("MYSQL_TEST_PASSWORD")?;
    let config: Config = serde_saphyr::from_str(&format!(
        "mysql:\n  host: 127.0.0.1\n  port: {port}\n  user: root\n  password_env: MYSQL_TEST_PASSWORD\nbackup:\n  databases: [app]\n"
    ))?;
    db::connect(&config, &password)
}

#[test]
fn mysql_modules_work_with_relational_data() -> Result<()> {
    let mut conn = connect()?;
    let suffix = Uuid::new_v4().simple().to_string();
    let schema = format!("mm_{}", &suffix[..12]);
    conn.query_drop("CREATE DATABASE IF NOT EXISTS app")?;
    conn.query_drop(format!("CREATE DATABASE `{schema}`"))?;

    for statement in include_str!("fixtures/relational_load.sql").split(';') {
        if !statement.trim().is_empty() {
            conn.query_drop(statement)?;
        }
    }
    let expected_counts = [
        ("organizations", 100),
        ("users", 5_000),
        ("categories", 200),
        ("products", 4_000),
        ("warehouses", 100),
        ("inventory", 8_000),
        ("orders", 8_000),
        ("order_items", 24_000),
        ("payments", 8_000),
        ("shipments", 6_000),
    ];
    for (table, expected) in expected_counts {
        let count: u64 = conn
            .query_first(format!("SELECT COUNT(*) FROM app.`{table}`"))?
            .ok_or("table count missing")?;
        assert_eq!(count, expected, "table {table}");
    }
    let foreign_keys: u64 = conn
        .exec_first(
            "SELECT COUNT(*) FROM information_schema.REFERENTIAL_CONSTRAINTS WHERE CONSTRAINT_SCHEMA = ?",
            ("app",),
        )?
        .ok_or("foreign key count missing")?;
    assert_eq!(foreign_keys, 10);

    assert_eq!(db::metadata_schema(&mut conn)?, schema);
    db::setup(&mut conn, &schema)?;
    db::setup(&mut conn, &schema)?;

    let databases = vec!["app".to_owned()];
    db::start_run(&mut conn, &schema, "test-success", &databases)?;
    let manifest = Manifest {
        id: "test-success".to_owned(),
        created_at: Utc::now(),
        completed_at: Utc::now(),
        databases,
        metadata_schema: schema.clone(),
        source_server_uuid: conn
            .query_first("SELECT @@server_uuid")?
            .ok_or("server UUID missing")?,
        binlog_file: "mysql-bin.000001".to_owned(),
        binlog_position: 4,
        bytes: 123,
        sha256: "a".repeat(64),
        pitr_scope_complete: true,
    };
    db::complete_run(&mut conn, &schema, &manifest, "/tmp/test-snapshot")?;
    let status: String = conn
        .exec_first(
            format!("SELECT status FROM `{schema}`.backup_runs WHERE id=?"),
            ("test-success",),
        )?
        .ok_or("completed run missing")?;
    assert_eq!(status, "succeeded");
    db::prune_run(&mut conn, &schema, "test-success")?;
    db::start_run(&mut conn, &schema, "test-failure", &["app".to_owned()])?;
    db::fail_run(&mut conn, &schema, "test-failure", "expected test error")?;
    let statuses: Vec<(String, String)> = conn.query(format!(
        "SELECT id, status FROM `{schema}`.backup_runs ORDER BY id"
    ))?;
    assert_eq!(
        statuses,
        vec![
            ("test-failure".to_owned(), "failed".to_owned()),
            ("test-success".to_owned(), "pruned".to_owned()),
        ]
    );

    let logs = binlog::remote_logs(&mut conn)?;
    assert!(!logs.is_empty());
    assert!(logs.iter().all(|(_, size)| *size > 0));

    let temp = tempfile::tempdir()?;
    let error_log = temp.path().join("error.log");
    fs::write(
        &error_log,
        b"2026-10-15T00:00:00.000000Z [Note] integration test line\n",
    )?;
    let (errors, _) = db::ingest_logs(&mut conn, &schema, &error_log)?;
    assert_eq!(errors, 1);
    let (errors_again, _) = db::ingest_logs(&mut conn, &schema, &error_log)?;
    assert_eq!(errors_again, 0);

    Ok(())
}
