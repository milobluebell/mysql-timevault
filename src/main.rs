use fs2::FileExt;
use mysql_timevault::{backup, binlog, config, db, private_directory, restore, Result};
use std::{
    env,
    fs::OpenOptions,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

fn main() {
    if let Err(err) = run() {
        eprintln!("fatal: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    let mut config_path = PathBuf::from("mm-config.yaml");
    let mut storage_dir = None;
    let mut command = String::from("serve");
    let mut rest = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => config_path = PathBuf::from(args.next().ok_or("--config needs a path")?),
            "--storage-dir" => {
                storage_dir = Some(PathBuf::from(
                    args.next().ok_or("--storage-dir needs a path")?,
                ))
            }
            "--once" => command = String::from("once"),
            "--help" | "-h" => {
                usage();
                return Ok(());
            }
            "serve" | "once" | "list" | "restore" if rest.is_empty() && command == "serve" => {
                command = arg
            }
            _ => rest.push(arg),
        }
    }
    let (config, config_dir) = config::load(&config_path)?;
    let root = match storage_dir {
        Some(path) => {
            let path = path.canonicalize()?;
            if !path.is_dir() {
                return Err("--storage-dir must be a directory".into());
            }
            path
        }
        None => config_dir,
    };
    if command == "list" {
        if !rest.is_empty() {
            return Err("list takes no arguments".into());
        }
        for m in backup::list(&root)? {
            println!(
                "{}  {}  databases={}  bytes={}  pitr={}",
                m.id,
                m.created_at,
                m.databases.join(","),
                m.bytes,
                if m.pitr_scope_complete {
                    "eligible"
                } else {
                    "unavailable"
                }
            );
        }
        return Ok(());
    }

    private_directory(&root.join("backups"))?;
    let _lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join(".backup.lock"))?;
    _lock
        .try_lock_exclusive()
        .map_err(|err| format!("another maintenance process holds mm-storage lock: {err}"))?;

    if command == "restore" {
        return restore::run(&root, &rest);
    }
    if !rest.is_empty() {
        return Err(format!("unexpected arguments: {}", rest.join(" ")).into());
    }
    let credentials = config.credentials()?;
    let mut conn = db::connect(&config, &credentials)?;
    let schema = db::metadata_schema(&mut conn, &root)?;
    db::setup(&mut conn, &schema)?;
    backup::remove_stale_partials(&root)?;
    db::reconcile_runs(&mut conn, &schema, &root)?;
    drop(conn);

    if command == "once" {
        ingest(&config, &credentials, &schema);
        let manifest = backup::run(&config, &credentials, &root, &schema)?;
        println!("backup completed: {}", manifest.id);
        return Ok(());
    }

    let stopping = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&stopping);
    ctrlc::set_handler(move || signal.store(true, Ordering::SeqCst))?;
    let mut next_backup = Instant::now() + Duration::from_secs(config.backup.interval_seconds);
    let mut next_binlog = Instant::now();
    let mut next_logs = Instant::now();
    println!(
        "maintenance service started; config={}",
        config_path.display()
    );
    while !stopping.load(Ordering::SeqCst) {
        let now = Instant::now();
        if now >= next_logs {
            ingest(&config, &credentials, &schema);
            next_logs = Instant::now() + Duration::from_secs(config.backup.log_poll_seconds);
        }
        if now >= next_backup {
            match backup::run(&config, &credentials, &root, &schema) {
                Ok(m) => println!("backup completed: {}", m.id),
                Err(err) => eprintln!("backup failed: {err}"),
            }
            next_backup = Instant::now() + Duration::from_secs(config.backup.interval_seconds);
            next_binlog = Instant::now() + Duration::from_secs(config.backup.binlog_poll_seconds);
        } else if now >= next_binlog {
            match backup::oldest_binlog(&root) {
                Ok(Some(file)) => match binlog::sync(&config, &credentials, &root, &file) {
                    Ok(count) if count > 0 => println!("binary logs synchronized: {count} files"),
                    Ok(_) => (),
                    Err(err) => eprintln!("binary log sync failed: {err}"),
                },
                Ok(None) => (),
                Err(err) => eprintln!("cannot find oldest backup: {err}"),
            }
            next_binlog = Instant::now() + Duration::from_secs(config.backup.binlog_poll_seconds);
        }
        thread::sleep(Duration::from_secs(1));
    }
    println!("maintenance service stopped");
    Ok(())
}

fn ingest(config: &config::Config, credentials: &config::Credentials, schema: &str) {
    match db::connect(config, credentials).and_then(|mut conn| {
        db::ingest_logs(&mut conn, schema, config.mysql.error_log_file.as_deref())
    }) {
        Ok((errors, slow)) if errors + slow > 0 => {
            println!("logs archived: {errors} errors, {slow} slow queries")
        }
        Ok(_) => (),
        Err(err) => eprintln!("log archiving failed: {err}"),
    }
}

fn usage() {
    println!("Usage: mysql-timevault [--config PATH] [--storage-dir PATH] [serve|once|list|restore ID --target-host HOST --target-user USER --target-password-env ENV [--target-port PORT] [--at RFC3339]]");
}
