# Maintaining MySQL

定期备份指定的 MySQL **数据库**，保留可选择的备份版本，并支持从完整备份加 binary log 恢复到指定时间点。

## 兼容性

- 支持 MySQL 5.7、8.0、8.4 作为备份源；恢复目标应与备份源保持相同主版本，且必须是独立的空实例。默认 Compose 使用 MySQL 8.4。MySQL 5.7 官方镜像仅提供 `linux/amd64`，在 ARM 主机上需要仿真运行。
- 5.7 使用 `compose.mysql57.yaml`；8.0 可设置 `.env` 中的 `MYSQL_IMAGE=mysql:8.0`；8.4 无需额外设置。项目镜像按服务端版本选用 8.0 或 8.4 的 `mysqldump`，并自带 `mysqlbinlog`；运行目录中仍需要可读取的 MySQL 错误日志文件。
- 事务表（如 InnoDB）可使用一致性快照；备份期间避免表结构 DDL。时间点恢复仅在配置覆盖服务器上全部非系统数据库时可用。本项目不备份 MySQL 系统库中的账号和权限。

## 功能

- 按库导出 SQL 压缩快照，记录 SHA-256、binary log 坐标、版本 ID 和任务状态；默认每 24 小时备份一次，最多保留 3 份。服务启动后等待首个备份周期，重启会重新计时；`once` 命令可立即备份。
- 持续复制 binary log，支持选定版本的完整恢复与 UTC 时间点恢复。
- 自动创建随机命名的 `mm_<12位十六进制>` 元数据库，保存备份工作记录、错误日志行和慢查询记录。
- MySQL 自身数据和日志、备份快照与 binary log 分别保存在两个宿主机目录，默认是 `./storage/mysql` 与 `./mm-storage`；备份目录权限设为仅服务用户可读写。

## 使用

1. 复制 `.env.example` 为 `.env`，将其权限设为 `600`，并填入三个不同的强密码。`MYSQL_STORAGE_DIR`、`MM_STORAGE_DIR`、`MM_CONFIG_FILE` 可分别指定宿主机上的 MySQL 数据目录、备份目录和配置文件；默认使用 Compose 文件旁的路径。若修改 `MM_STORAGE_DIR`，也请设置 `MM_CONFIG_FILE`。编辑配置文件的 `backup.databases`，列出需要备份的数据库；`backup.interval_seconds` 默认是 `86400`。新增数据库时，给备份账号授予该库导出所需权限。
2. 创建目录并启动服务：

```bash
mkdir -p storage/mysql mm-storage/backups
docker compose up -d --build
```

自定义路径时，先创建对应的宿主机目录，并确保 `MM_CONFIG_FILE` 是已存在的文件；路径变量修改后重新运行 `docker compose up -d` 以重建容器。使用 MySQL 5.7 时，改用 `docker compose -f compose.yaml -f compose.mysql57.yaml up -d --build`。Linux 上确保 `.env` 中的 `MM_UID:MM_GID` 对 `MM_STORAGE_DIR` 可写，且备份容器能通过 MySQL 组读取 `MYSQL_STORAGE_DIR/error.log`；示例 Compose 添加了组 ID `999`。MySQL 的 `24075` 默认只对宿主机本地开放；需要远程连接时自行调整端口绑定并配置防火墙。配置文件内容修改后运行 `docker compose restart backup`。

查看版本及立即备份：

```bash
docker compose run --rm --no-deps backup list
docker compose stop backup
docker compose run --rm --no-deps backup once
docker compose start backup
```

恢复时，先停止备份服务，并准备全新的独立目标目录（默认 `./storage/mysql-restore`，可用 `MYSQL_RESTORE_STORAGE_DIR` 调整）。把 `.env` 的 `MYSQL_RESTORE_IMAGE` 设为与源相同的主版本（8.0 可设 `mysql:8.0`）；5.7 使用下面命令后追加说明中的覆盖文件。随后运行：

```bash
docker compose stop backup
docker compose -f compose.yaml -f compose.restore.yaml up -d --wait mysql_restore
docker compose -f compose.yaml -f compose.restore.yaml run --rm --no-deps backup \
  restore <版本ID> --target-host mysql_restore --target-user root \
  --target-password-env MYSQL_RESTORE_ROOT_PASSWORD
```

恢复 MySQL 5.7 时，两条恢复命令都再添加 `-f compose.restore57.yaml`。它会把独立目标设置为 5.7，并在 ARM 主机上使用 `linux/amd64` 仿真。

要恢复到指定 UTC 时间，在 `restore` 命令后添加 `--at 2026-10-15T12:00:00Z`。该时间为**不包含**的整秒上界，且不能晚于 `mm-storage/binlogs/coverage.json` 的 `covered_through`。恢复前会校验快照与 binary log 连续性，拒绝向源实例或非空目标导入。

许可证：[0BSD](LICENSE)。
