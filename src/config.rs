use serde::Deserialize;
use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

use crate::Result;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub mysql: MysqlConfig,
    pub backup: BackupConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MysqlConfig {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: String,
    pub password_env: String,
    #[serde(default)]
    pub error_log_file: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupConfig {
    pub databases: Vec<String>,
    #[serde(default = "default_backup_interval")]
    pub interval_seconds: u64,
    #[serde(default = "default_max_backups")]
    pub max_backups: usize,
    #[serde(default = "default_binlog_poll")]
    pub binlog_poll_seconds: u64,
    #[serde(default = "default_log_poll")]
    pub log_poll_seconds: u64,
}

fn default_port() -> u16 {
    3306
}
fn default_max_backups() -> usize {
    3
}
fn default_backup_interval() -> u64 {
    24 * 60 * 60
}
fn default_binlog_poll() -> u64 {
    60
}
fn default_log_poll() -> u64 {
    10
}

pub fn load(path: &Path) -> Result<(Config, PathBuf)> {
    let config: Config = serde_saphyr::from_str(&fs::read_to_string(path)?)?;
    config.validate()?;
    Ok((config, config_directory(path)?))
}

fn config_directory(path: &Path) -> Result<PathBuf> {
    Ok(path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()?)
}

impl Config {
    fn validate(&self) -> Result<()> {
        if self.mysql.host.trim().is_empty()
            || self.mysql.user.trim().is_empty()
            || self.mysql.password_env.trim().is_empty()
            || self.mysql.port == 0
        {
            return Err("mysql host, port, user, and password_env are required".into());
        }
        if self.backup.databases.is_empty()
            || self.backup.interval_seconds == 0
            || self.backup.max_backups == 0
            || self.backup.binlog_poll_seconds == 0
            || self.backup.log_poll_seconds == 0
        {
            return Err("databases and all backup counts/intervals must be nonzero".into());
        }
        let mut seen = HashSet::new();
        for database in &self.backup.databases {
            validate_identifier(database)?;
            if matches!(
                database.as_str(),
                "mysql" | "sys" | "information_schema" | "performance_schema"
            ) || database.starts_with("mm_")
            {
                return Err(format!(
                    "database {database} is reserved; the metadata database is added automatically"
                )
                .into());
            }
            if !seen.insert(database) {
                return Err(format!("duplicate database: {database}").into());
            }
        }
        Ok(())
    }

    pub fn password(&self) -> Result<String> {
        let value = env::var(&self.mysql.password_env)
            .map_err(|_| format!("missing environment variable {}", self.mysql.password_env))?;
        if value.is_empty() {
            return Err("MySQL password is empty".into());
        }
        Ok(value)
    }
}

pub fn validate_identifier(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(format!(
            "invalid database name {value:?}; use 1–64 ASCII letters, digits, or underscores"
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_backup_interval_and_retention() {
        let config: Config = serde_saphyr::from_str("mysql:\n  host: mysql\n  user: mm_agent\n  password_env: MYSQL_PASSWORD\nbackup:\n  databases: [app]\n").unwrap();
        assert_eq!(config.backup.interval_seconds, 86_400);
        assert_eq!(config.backup.max_backups, 3);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn metadata_schema_is_not_user_selected() {
        let config: Config = serde_saphyr::from_str("mysql:\n  host: mysql\n  user: mm_agent\n  password_env: MYSQL_PASSWORD\nbackup:\n  databases: [mm_0123456789ab]\n  interval_seconds: 3600\n").unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn config_filename_in_current_directory_resolves_parent() {
        assert_eq!(
            config_directory(Path::new("mm-config.yaml")).unwrap(),
            std::env::current_dir().unwrap()
        );
    }
}
