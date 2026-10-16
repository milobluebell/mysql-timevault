# MySQL TimeVault

Scheduled MySQL backups and binary-log point-in-time recovery. [简体中文](README.zh-CN.md) · License: [0BSD](LICENSE).

## Start

1. Copy `.env.example` to `.env`; set `MM_IMAGE` (`0.1.1-amd64` or `0.1.1-arm64`) and `MYSQL_PASSWORD`. Edit `mm-config.yaml` for your source and databases.
2. Create a source account with the [grants below](#source-mysql-permissions), then start:

```bash
chmod 600 .env
mkdir -p mm-storage/backups
docker compose up -d
```

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

- Sources: MySQL 5.7, 8.0, 8.4. Restore to a separate, empty server of the same series. The restore account needs privileges for the snapshot and replayed events; accounts and grants are not backed up.
- Enable binary logs with `binlog_format=ROW`. Point-in-time recovery needs all non-system databases and continuous logs; use `--at <RFC3339 UTC time>`. Set `RESTORE_PASSWORD`; use `--target-port` if needed.
- The source must be reachable from the container: use `host.docker.internal` for the Docker host or set `MYSQL_DOCKER_NETWORK` and `COMPOSE_FILE=compose.yaml:compose.network.yaml` for another Compose project.
- `MM_STORAGE_DIR` defaults to `./mm-storage`; `MM_CONFIG_FILE` to `./mm-config.yaml`. First scheduled backup: 24 hours; retained backups: 3. Restart after YAML changes and keep an off-host backup copy.
- Bundled MySQL: copy `examples/mm-config.with-mysql.yaml` to `mm-config.yaml`, then run `docker compose -f compose.with-mysql.yaml up -d`. Point `MYSQL_STORAGE_DIR` at the existing data directory when reusing an instance; it defaults to `./storage`.
- Slow-log collection needs `slow_query_log=ON` and `log_output=TABLE`. Error-log archiving needs `MYSQL_ERROR_LOG_FILE`, `mysql.error_log_file: /mysql-data/error.log`, and `compose.error-log.yaml`.
- On 8.x, the `mm\_%` grant needs `partial_revokes=OFF`; database-name wildcards are deprecated. Global `SELECT` for routine dumps lets the account read all source databases.
