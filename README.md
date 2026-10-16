# MySQL TimeVault

Scheduled MySQL backups and binary-log point-in-time recovery. [简体中文](README.zh-CN.md) · License: [0BSD](LICENSE).

## Start

Edit `mm-config.yaml` for the source server and databases, then create a source account with the [grants below](#source-mysql-permissions). Set `mysql.user` and `mysql.password` in YAML, or omit either field to read `MYSQL_USER` or `MYSQL_PASSWORD` from the process environment. YAML takes precedence for each field.

The executable reads `mm-config.yaml` from its working directory by default. Run it without Docker:

```bash
cargo build --release
./target/release/mysql-timevault once
./target/release/mysql-timevault serve
```

`--config PATH` selects another YAML file; `--storage-dir PATH` selects a separate backup directory. `list` and `restore` use the same executable.

For Docker deployment, copy `.env.example` to `.env`, set `MM_IMAGE` (`0.1.1-amd64` or `0.1.1-arm64`), and start:

```bash
chmod 600 .env
mkdir -p mm-storage/backups
docker compose up -d
```

Compose forwards `MYSQL_USER` and `MYSQL_PASSWORD` when set in `.env`; neither is required when both credentials are in YAML.

## Source MySQL permissions

After creating `mm_agent`, replace `app` with each selected database and `<backup-host>` with its client host:

```sql
GRANT RELOAD, REPLICATION CLIENT, REPLICATION SLAVE ON *.* TO 'mm_agent'@'<backup-host>';
GRANT SHOW VIEW, TRIGGER, EVENT ON `app`.* TO 'mm_agent'@'<backup-host>';
GRANT CREATE, INSERT, UPDATE ON `mm\_%`.* TO 'mm_agent'@'<backup-host>';
```

For MySQL 5.7, also grant scoped reads:

```sql
GRANT SELECT ON `app`.* TO 'mm_agent'@'<backup-host>';
GRANT SELECT ON `mysql`.`slow_log` TO 'mm_agent'@'<backup-host>';
GRANT SELECT ON `mysql`.`proc` TO 'mm_agent'@'<backup-host>';
GRANT SELECT ON `mm\_%`.* TO 'mm_agent'@'<backup-host>';
```

For MySQL 8.0 and 8.4, `mysqldump --routines` requires global `SELECT`:

```sql
GRANT SELECT ON *.* TO 'mm_agent'@'<backup-host>';
```

## Backup and restore

Stop `serve` before running `once` or `restore` against the same storage directory. For native use:

```bash
./target/release/mysql-timevault once
./target/release/mysql-timevault list
./target/release/mysql-timevault restore <backup-id> \
  --target-host <empty-mysql-host> --target-user <restore-user> \
  --target-password-env RESTORE_PASSWORD
```

The same commands can run in the backup container:

```bash
docker compose stop backup
docker compose run --rm backup once
docker compose run --rm backup list
docker compose run --rm -e RESTORE_PASSWORD backup \
  restore <backup-id> --target-host <empty-mysql-host> \
  --target-user <restore-user> --target-password-env RESTORE_PASSWORD
docker compose start backup
```

## Notes

- Keep a YAML file containing `mysql.password` private, for example with `chmod 600 mm-config.yaml`. Native runs need compatible `mysqldump`/`mysqldump80` and `mysqlbinlog` clients on `PATH`.
- Sources: MySQL 5.7, 8.0, 8.4. Restore to a separate, empty server of the same series. The restore account needs privileges for the snapshot and replayed events; accounts and grants are not backed up.
- Enable binary logs with `binlog_format=ROW`. Point-in-time recovery needs all non-system databases and continuous logs; use `--at <RFC3339 UTC time>`. Set `RESTORE_PASSWORD`; use `--target-port` if needed.
- For Docker, the source must be reachable from the container: use `host.docker.internal` for the Docker host or set `MYSQL_DOCKER_NETWORK` and `COMPOSE_FILE=compose.yaml:compose.network.yaml` for another Compose project.
- Docker's `MM_STORAGE_DIR` defaults to `./mm-storage`; `MM_CONFIG_FILE` to `./mm-config.yaml`. First scheduled backup: 24 hours; retained backups: 3. Restart after YAML changes and keep an off-host backup copy.
- Bundled MySQL: copy `examples/mm-config.with-mysql.yaml` to `mm-config.yaml`, then run `docker compose -f compose.with-mysql.yaml up -d`. The backup container connects to `mysql:3306` within the same Compose project; `27051` is for host or external clients. Point `MYSQL_STORAGE_DIR` at the existing data directory when reusing an instance; it defaults to `./storage`.
- Slow-log collection needs `slow_query_log=ON` and `log_output=TABLE`. Error-log archiving needs `MYSQL_ERROR_LOG_FILE`, `mysql.error_log_file: /mysql-data/error.log`, and `compose.error-log.yaml`.
- On 8.x, the `mm\_%` grant needs `partial_revokes=OFF`; database-name wildcards are deprecated. Global `SELECT` for routine dumps lets the account read all source databases.
