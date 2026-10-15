use chrono::Utc;
use mysql_timevault::backup::Manifest;
use std::{fs, process::Command};

#[test]
fn list_uses_storage_dir_even_when_config_is_elsewhere() {
    let temp = tempfile::tempdir().unwrap();
    let config_dir = temp.path().join("config");
    let storage_dir = temp.path().join("storage");
    fs::create_dir(&config_dir).unwrap();
    let id = "20261016T000000Z-12345678";
    let backup_dir = storage_dir.join("backups").join(id);
    fs::create_dir_all(&backup_dir).unwrap();
    let config_path = config_dir.join("mm-config.yaml");
    fs::write(
        &config_path,
        "mysql:\n  host: mysql\n  user: backup\n  password_env: MYSQL_PASSWORD\nbackup:\n  databases: [app]\n",
    )
    .unwrap();
    let manifest = Manifest {
        id: id.to_owned(),
        created_at: Utc::now(),
        completed_at: Utc::now(),
        databases: vec!["app".to_owned()],
        metadata_schema: "mm_0123456789ab".to_owned(),
        source_server_uuid: "source".to_owned(),
        binlog_file: "mysql-bin.000001".to_owned(),
        binlog_position: 4,
        bytes: 123,
        sha256: "a".repeat(64),
        pitr_scope_complete: false,
    };
    fs::write(
        backup_dir.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_mysql-timevault"))
        .arg("--config")
        .arg(config_path)
        .arg("--storage-dir")
        .arg(storage_dir)
        .arg("list")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8(output.stdout).unwrap().contains(id));
}
