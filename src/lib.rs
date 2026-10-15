pub mod backup;
pub mod binlog;
pub mod config;
pub mod db;
pub mod restore;

use std::{error::Error, fs, path::Path};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub fn private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
