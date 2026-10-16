# MySQL TimeVault

定时备份 MySQL，并通过 binary log 恢复到指定时间。[English](README.md) · 协议：[0BSD](LICENSE)。

## 启动

在 `mm-config.yaml` 中填写源库和待备份库名，并按[下节](#源库最小权限)创建、授权源库账号。可在 YAML 中填写 `mysql.user` 和 `mysql.password`；任一字段未填写时，分别读取进程环境中的 `MYSQL_USER` 或 `MYSQL_PASSWORD`。每个字段都以 YAML 为优先。

程序默认读取运行目录的 `mm-config.yaml`，不需要 Docker：

```bash
cargo build --release
./target/release/mysql-timevault once
./target/release/mysql-timevault serve
```

可用 `--config PATH` 指定其他 YAML 文件，用 `--storage-dir PATH` 指定独立备份目录；`list` 和 `restore` 也可直接运行。

使用 Docker 部署时，将 `.env.example` 复制为 `.env`，填写 `MM_IMAGE`（`0.1.1-amd64` 或 `0.1.1-arm64`），然后启动：

```bash
chmod 600 .env
mkdir -p mm-storage/backups
docker compose up -d
```

Compose 会将 `.env` 中的 `MYSQL_USER`、`MYSQL_PASSWORD` 传给程序；如果 YAML 已填写账号和密码，则无需设置这两个环境变量。

## 源库最小权限

创建 `mm_agent` 后，将 `app` 替换为每个待备份库名，`<backup-host>` 替换为客户端主机：

```sql
GRANT RELOAD, REPLICATION CLIENT, REPLICATION SLAVE ON *.* TO 'mm_agent'@'<backup-host>';
GRANT SHOW VIEW, TRIGGER, EVENT ON `app`.* TO 'mm_agent'@'<backup-host>';
GRANT CREATE, INSERT, UPDATE ON `mm\_%`.* TO 'mm_agent'@'<backup-host>';
```

MySQL 5.7 另需按对象授予读取权限：

```sql
GRANT SELECT ON `app`.* TO 'mm_agent'@'<backup-host>';
GRANT SELECT ON `mysql`.`slow_log` TO 'mm_agent'@'<backup-host>';
GRANT SELECT ON `mysql`.`proc` TO 'mm_agent'@'<backup-host>';
GRANT SELECT ON `mm\_%`.* TO 'mm_agent'@'<backup-host>';
```

MySQL 8.0 和 8.4 的 `mysqldump --routines` 要求全局 `SELECT`：

```sql
GRANT SELECT ON *.* TO 'mm_agent'@'<backup-host>';
```

## 备份与恢复

对同一备份目录执行 `once` 或 `restore` 前，先停止运行中的 `serve`。本机运行：

```bash
./target/release/mysql-timevault once
./target/release/mysql-timevault list
./target/release/mysql-timevault restore <备份ID> \
  --target-host <空实例地址> --target-user <恢复账号> \
  --target-password-env RESTORE_PASSWORD
```

备份容器也可执行相同命令：

```bash
docker compose stop backup
docker compose run --rm backup once
docker compose run --rm backup list
docker compose run --rm -e RESTORE_PASSWORD backup \
  restore <备份ID> --target-host <空实例地址> \
  --target-user <恢复账号> --target-password-env RESTORE_PASSWORD
docker compose start backup
```

## 注意事项

- YAML 写有 `mysql.password` 时应限制文件权限，例如 `chmod 600 mm-config.yaml`。本机运行需在 `PATH` 中提供兼容的 `mysqldump`／`mysqldump80` 和 `mysqlbinlog` 客户端。
- 备份源支持 MySQL 5.7、8.0、8.4。恢复目标须为同系列的独立空实例；恢复账号需能执行快照及重放事件。账号和授权不会被备份。
- 源库须启用 binary log 和 `binlog_format=ROW`。时间点恢复要求备份全部非系统库及连续日志；用 `--at <RFC3339 UTC 时间>` 指定时间。恢复前设置 `RESTORE_PASSWORD`；必要时加 `--target-port`。
- 使用 Docker 时，源库须从容器内可达：宿主机用 `host.docker.internal`；另一 Compose 项目需设置 `MYSQL_DOCKER_NETWORK` 和 `COMPOSE_FILE=compose.yaml:compose.network.yaml`。
- Docker 的 `MM_STORAGE_DIR` 默认为 `./mm-storage`；`MM_CONFIG_FILE` 默认为 `./mm-config.yaml`。首次定时备份在 24 小时后，默认保留 3 份；修改 YAML 后重启，并异机保存备份副本。
- 自带 MySQL：将 `examples/mm-config.with-mysql.yaml` 复制到 `mm-config.yaml`，运行 `docker compose -f compose.with-mysql.yaml up -d`。同一 Compose 中备份容器连接 `mysql:3306`；`27051` 是提供给宿主机或外部客户端的端口。复用旧实例须让 `MYSQL_STORAGE_DIR` 指向原数据目录，默认值为 `./storage`。
- 慢查询采集需启用 `slow_query_log=ON` 和 `log_output=TABLE`；错误日志归档需设置 `MYSQL_ERROR_LOG_FILE`、`mysql.error_log_file: /mysql-data/error.log` 和 `compose.error-log.yaml`。
- 在 8.x 中，`mm\_%` 授权要求 `partial_revokes=OFF`，且库名通配符已弃用。导出例程所需的全局 `SELECT` 允许读取源实例的所有库。
