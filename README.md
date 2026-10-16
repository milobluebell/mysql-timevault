# MySQL TimeVault

Scheduled, versioned database backups with binary-log point-in-time recovery. [简体中文](README.zh-CN.md)

## Compatibility

MySQL 5.7, 8.0 and 8.4 are supported as sources. The container runs only the backup client; bring your own MySQL server. Restores require a separate, empty MySQL server of the same release series. MySQL accounts and grants are not backed up.

Choose an image tag matching your host: `0.1.1-amd64` or `0.1.1-arm64` (also `latest-amd64` / `latest-arm64`). Each tag contains one architecture.

## Start

1. Make the source reachable by TCP. Enable binary logging with `binlog_format=ROW`, and keep binary logs longer than the longest expected outage. Give the backup account read/dump privileges on every selected database, access to `mysql.slow_log`, binary-log replication privileges, and permission to create and manage `mm_<random>` databases. For slow-query collection, enable `slow_query_log=ON` and `log_output=TABLE`.
2. Copy `.env.example` to `.env`. Set `MM_IMAGE` to your CPU architecture and set `MYSQL_PASSWORD`. Edit `mm-config.yaml`: set `mysql.host`, `mysql.port`, `mysql.user`, and `backup.databases`. The host must be reachable **from the container**. For MySQL on the Docker host, use `host.docker.internal` and its exposed port. For MySQL in another Compose project, set `MYSQL_DOCKER_NETWORK` to its existing network, set `COMPOSE_FILE=compose.yaml:compose.network.yaml` in `.env`, and use its container name. `localhost` refers to TimeVault itself.
3. Run:

```bash
chmod 600 .env
mkdir -p mm-storage/backups
docker compose up -d
```

`MM_STORAGE_DIR` and `MM_CONFIG_FILE` in `.env` select the host backup directory and YAML file. The YAML file must already exist. The first scheduled backup runs after 24 hours by default; `backup.max_backups` defaults to 3. Restart the service after editing the YAML file.

To start MySQL 8.4 in the same Compose project, copy `examples/mm-config.with-mysql.yaml` to `mm-config.yaml` and run `docker compose -f compose.with-mysql.yaml up -d`. `MYSQL_STORAGE_DIR` selects the MySQL data directory.

The error log is a **server-side file**, so remote MySQL hosts cannot expose it over the SQL connection. To archive it when the file is available on the Docker host, set `MYSQL_ERROR_LOG_FILE` to its absolute host path, add `mysql.error_log_file: /mysql-data/error.log` to the YAML, and append `compose.error-log.yaml` to `COMPOSE_FILE` (or pass both files with `-f`). Without that mount, slow-query records and backups still run.

## Backup and restore

```bash
docker compose stop backup
docker compose run --rm backup once
docker compose run --rm backup list
docker compose run --rm -e RESTORE_PASSWORD backup \
  restore <backup-id> --target-host <empty-mysql-host> \
  --target-user root --target-password-env RESTORE_PASSWORD
docker compose start backup
```

Set `RESTORE_PASSWORD` in your shell before restoring. Add `--target-port <port>` if needed, or `--at 2026-10-15T12:00:00Z` for UTC point-in-time recovery. Point-in-time recovery requires backups of **all** non-system databases and continuous binary-log coverage. Keep an off-host copy of `MM_STORAGE_DIR`.

License: [0BSD](LICENSE).
