#!/bin/bash
set -euo pipefail

if [[ ! "${MYSQL_USER:-}" =~ ^[A-Za-z0-9_]+$ ]]; then
  echo "MYSQL_USER must contain only ASCII letters, digits, or underscores" >&2
  exit 1
fi

suffix=$(tr -d '-' </proc/sys/kernel/random/uuid | cut -c1-12)
schema_name="mm_${suffix}"

if [[ ! "$schema_name" =~ ^mm_[0-9a-f]{12}$ ]]; then
  echo "invalid metadata schema name: $schema_name" >&2
  exit 1
fi

MYSQL_PWD="$MYSQL_ROOT_PASSWORD" mysql --protocol=socket -uroot <<SQL
CREATE DATABASE IF NOT EXISTS \`$schema_name\` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
GRANT ALL PRIVILEGES ON \`$schema_name\`.* TO '$MYSQL_USER'@'%';
GRANT SELECT ON mysql.slow_log TO '$MYSQL_USER'@'%';
GRANT RELOAD, REPLICATION CLIENT, REPLICATION SLAVE, SHOW DATABASES ON *.* TO '$MYSQL_USER'@'%';
SQL

echo "Maintaining MySQL metadata schema: $schema_name"
