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
    pub user: Option<String>,
    pub password: Option<String>,
    #[serde(default)]
    pub error_log_file: Option<PathBuf>,
}

pub struct Credentials {
    pub user: String,
    pub password: String,
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
        if self.mysql.host.trim().is_empty() || self.mysql.port == 0 {
            return Err("mysql host and port are required".into());
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

    pub fn credentials(&self) -> Result<Credentials> {
        self.mysql.credentials_with_env(|name| env::var(name).ok())
    }
}

impl MysqlConfig {
    fn credentials_with_env(
        &self,
        get_env: impl Fn(&str) -> Option<String>,
    ) -> Result<Credentials> {
        let user = self
            .user
            .clone()
            .or_else(|| get_env("MYSQL_USER"))
            .filter(|value| !value.trim().is_empty())
            .ok_or("mysql.user or MYSQL_USER is required")?;
        let password = self
            .password
            .clone()
            .or_else(|| get_env("MYSQL_PASSWORD"))
            .filter(|value| !value.is_empty())
            .ok_or("mysql.password or MYSQL_PASSWORD is required")?;
        Ok(Credentials { user, password })
    }
}

pub fn validate_identifier(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || value.starts_with('-')
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(format!(
            "invalid database name {value:?}; use 1–64 ASCII letters, digits, underscores, or hyphens, without a leading hyphen"
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
        let config: Config = serde_saphyr::from_str("mysql:\n  host: mysql\n  user: mm_agent\n  password: secret\nbackup:\n  databases: [app]\n").unwrap();
        assert_eq!(config.backup.interval_seconds, 86_400);
        assert_eq!(config.backup.max_backups, 3);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn metadata_schema_is_not_user_selected() {
        let config: Config = serde_saphyr::from_str("mysql:\n  host: mysql\n  user: mm_agent\n  password: secret\nbackup:\n  databases: [mm_0123456789ab]\n  interval_seconds: 3600\n").unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn hyphenated_database_name_is_valid() {
        let config: Config = serde_saphyr::from_str("mysql:\n  host: mysql\n  user: mm_agent\n  password: secret\nbackup:\n  databases: [overjoy, overjoy_auth, zq-rag]\n").unwrap();
        assert!(config.validate().is_ok());
        assert!(validate_identifier("-option").is_err());
    }

    #[test]
    fn credentials_prefer_yaml_and_fall_back_independently() {
        let config: Config = serde_saphyr::from_str(
            "mysql:\n  host: mysql\n  user: yaml_user\nbackup:\n  databases: [app]\n",
        )
        .unwrap();
        let credentials = config
            .mysql
            .credentials_with_env(|name| match name {
                "MYSQL_USER" => Some("env_user".into()),
                "MYSQL_PASSWORD" => Some("env_password".into()),
                _ => None,
            })
            .unwrap();
        assert_eq!(credentials.user, "yaml_user");
        assert_eq!(credentials.password, "env_password");

        let config: Config = serde_saphyr::from_str(
            "mysql:\n  host: mysql\n  password: yaml_password\nbackup:\n  databases: [app]\n",
        )
        .unwrap();
        let credentials = config
            .mysql
            .credentials_with_env(|name| (name == "MYSQL_USER").then(|| "env_user".into()))
            .unwrap();
        assert_eq!(credentials.user, "env_user");
        assert_eq!(credentials.password, "yaml_password");
    }

    #[test]
    fn yaml_credentials_work_without_environment() {
        let config: Config = serde_saphyr::from_str(
            "mysql:\n  host: 127.0.0.1\n  user: local_user\n  password: local_password\nbackup:\n  databases: [app]\n",
        )
        .unwrap();
        let credentials = config.mysql.credentials_with_env(|_| None).unwrap();
        assert_eq!(credentials.user, "local_user");
        assert_eq!(credentials.password, "local_password");
    }

    #[test]
    fn environment_credentials_work_without_yaml_fields() {
        let config: Config =
            serde_saphyr::from_str("mysql:\n  host: mysql\nbackup:\n  databases: [app]\n").unwrap();
        let credentials = config
            .mysql
            .credentials_with_env(|name| match name {
                "MYSQL_USER" => Some("env_user".into()),
                "MYSQL_PASSWORD" => Some("env_password".into()),
                _ => None,
            })
            .unwrap();
        assert_eq!(credentials.user, "env_user");
        assert_eq!(credentials.password, "env_password");
    }

    #[test]
    fn missing_or_empty_credentials_fail_without_echoing_values() {
        let config: Config =
            serde_saphyr::from_str("mysql:\n  host: mysql\nbackup:\n  databases: [app]\n").unwrap();
        assert_eq!(
            config
                .mysql
                .credentials_with_env(|_| None)
                .err()
                .unwrap()
                .to_string(),
            "mysql.user or MYSQL_USER is required"
        );
        let config: Config = serde_saphyr::from_str(
            "mysql:\n  host: mysql\n  user: yaml_user\n  password: ''\nbackup:\n  databases: [app]\n",
        )
        .unwrap();
        assert_eq!(
            config
                .mysql
                .credentials_with_env(|_| Some("env_password".into()))
                .err()
                .unwrap()
                .to_string(),
            "mysql.password or MYSQL_PASSWORD is required"
        );
    }

    #[test]
    fn config_filename_in_current_directory_resolves_parent() {
        assert_eq!(
            config_directory(Path::new("mm-config.yaml")).unwrap(),
            std::env::current_dir().unwrap()
        );
    }
}
