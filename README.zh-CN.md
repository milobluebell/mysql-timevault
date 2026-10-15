# MySQL TimeVault

定时按库备份 MySQL，保留可选版本，并用 binary log 恢复到指定时间。[English](README.md)

## 兼容性

支持 MySQL 5.7、8.0、8.4 作为备份源。容器仅运行维护客户端，**不会启动 MySQL 服务端**。恢复目标须为相同版本系列的独立空实例；MySQL 账号与授权不在备份范围内。

按宿主机 CPU 选择镜像：`0.1.0-amd64` 或 `0.1.0-arm64`，也可选 `latest-amd64` / `latest-arm64`。每个标签只包含一种架构。

## 启动

1. 确保源库可通过 TCP 访问，启用 binary log 和 `binlog_format=ROW`，并设置足够长的日志保留期。备份账号需有目标库的读取/导出权限、binary log 复制权限、`mysql.slow_log` 读取权限，以及创建和管理 `mm_<随机值>` 库的权限。采集慢查询还需启用 `slow_query_log=ON`、`log_output=TABLE`。
2. 复制 `.env.example` 为 `.env`，按 CPU 设置 `MM_IMAGE` 并填写 `MYSQL_PASSWORD`。编辑 `mm-config.yaml` 的 `mysql.host`、`mysql.port`、`mysql.user` 和 `backup.databases`。地址必须**从维护容器内部可达**：宿主机上的 MySQL 可用 `host.docker.internal` 和它开放的端口；若属于另一 Compose 项目，在 `.env` 中填写其现有网络名 `MYSQL_DOCKER_NETWORK` 和 `COMPOSE_FILE=compose.yaml:compose.network.yaml`，并用其容器名连接。`localhost` 指维护容器自身。
3. 启动：

```bash
chmod 600 .env
mkdir -p mm-storage/backups
docker compose up -d
```

`.env` 中的 `MM_STORAGE_DIR` 和 `MM_CONFIG_FILE` 分别指定宿主机备份目录和 YAML 文件；YAML 文件需预先存在。Linux 上将 `MM_UID`、`MM_GID` 设为目录所有者。默认启动 24 小时后首次备份，最多保留 3 份；修改 YAML 后重启服务。

要在同一个 Compose 中启动 MySQL 8.4，可将 `examples/mm-config.with-mysql.yaml` 复制到 `mm-config.yaml`，并运行 `docker compose -f compose.with-mysql.yaml up -d`。`MYSQL_STORAGE_DIR` 可指定 MySQL 数据目录。

错误日志是 MySQL 服务端本地文件，不能仅靠 SQL 连接读取。如果 Docker 宿主机能访问它，设置 `MYSQL_ERROR_LOG_FILE` 为宿主机上的绝对路径，在 YAML 中加入 `mysql.error_log_file: /mysql-data/error.log`，并将 `compose.error-log.yaml` 加入 `COMPOSE_FILE`（或以 `-f` 指定）。未挂载时，备份和慢查询采集仍可运行。

## 备份与恢复

```bash
docker compose stop backup
docker compose run --rm backup once
docker compose run --rm backup list
docker compose run --rm -e RESTORE_PASSWORD backup \
  restore <备份ID> --target-host <空实例地址> \
  --target-user root --target-password-env RESTORE_PASSWORD
docker compose start backup
```

恢复前在 shell 中设置 `RESTORE_PASSWORD`。目标端口不同可加 `--target-port <端口>`；按 UTC 时间点恢复可加 `--at 2026-10-15T12:00:00Z`。时间点恢复要求备份覆盖全部非系统库，且 binary log 连续。请另存一份异机的 `MM_STORAGE_DIR`。

协议：[0BSD](LICENSE)。
