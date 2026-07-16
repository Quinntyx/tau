//! Filesystem path for the credentials TOML.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Default credentials file path: `$XDG_CONFIG_HOME/tau/credentials.toml`.
pub fn credentials_file_path() -> Result<PathBuf> {
    let base = dirs::config_dir().context("could not determine config directory")?;
    Ok(base.join("tau").join("credentials.toml"))
}
