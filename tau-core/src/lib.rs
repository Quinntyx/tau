//! tau — shared cross-crate helpers and domain foundations.
//!
//! Holds the small pieces of common infrastructure every crate needs: where
//! tau's runtime files live, and (as milestones land) config, credentials,
//! sessions, tools, and the agent runtime built on `rig`.

pub mod agent;
pub mod config;
pub mod credentials;
pub mod db;
pub mod permissions;
pub mod plan;
pub mod provider;
pub mod tools;

use std::path::PathBuf;

use anyhow::{Context, Result};

/// `~/.tau`, created if missing.
pub fn home_tau() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not determine home directory")?;
    let p = home.join(".tau");
    std::fs::create_dir_all(&p).with_context(|| format!("creating {}", p.display()))?;
    Ok(p)
}

/// Default daemon socket path: `$XDG_RUNTIME_DIR/tau.sock`, falling back to
/// `~/.tau/tau.sock` when no runtime directory is available.
pub fn default_socket_path() -> Result<PathBuf> {
    if let Some(rd) = dirs::runtime_dir() {
        return Ok(rd.join("tau.sock"));
    }
    Ok(home_tau()?.join("tau.sock"))
}
