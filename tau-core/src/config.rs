//! tau configuration (global, MVP).
//!
//! Minimal schema grown per-milestone. File: `~/.config/tau/config.toml`. All
//! fields optional; unknown fields are ignored for forward compatibility. The
//! per-project `.tau/` cascade is deferred with the project system.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Default model id when no session/agent model is selected.
    pub model: Option<String>,
    /// Default primary agent id (e.g. "plan", "code").
    pub default_agent: Option<String>,
    /// Per-provider overrides. Keyed by provider id (e.g. "anthropic").
    pub providers: BTreeMap<String, ProviderConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// Override the provider's API base URL.
    pub api_base: Option<String>,
    /// Name of the env var that holds this provider's API key (overrides the
    /// default `{PROVIDER}_API_KEY` convention).
    pub api_key_env: Option<String>,
}

/// Default config file path: `$XDG_CONFIG_HOME/tau/config.toml` (or
/// `~/.config/tau/config.toml`).
pub fn config_path() -> Result<PathBuf> {
    let base = dirs::config_dir().context("could not determine config directory")?;
    Ok(base.join("tau").join("config.toml"))
}

impl Config {
    /// Load config from the default path. Missing file ⇒ defaults (not an error).
    pub fn load() -> Result<Config> {
        let path = config_path()?;
        Self::load_from(&path)
    }

    /// Load config from an explicit path. Missing file ⇒ defaults.
    pub fn load_from(path: &Path) -> Result<Config> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let cfg: Config =
                    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
                Ok(cfg)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_default() {
        let cfg = Config::load_from(Path::new("/nonexistent/tau/config.toml")).unwrap();
        assert!(cfg.model.is_none());
        assert!(cfg.providers.is_empty());
    }

    #[test]
    fn parses_minimal() {
        let text = r#"
            model = "claude-opus"
            default_agent = "plan"

            [providers.anthropic]
            api_base = "https://custom.example.com"
            api_key_env = "MY_ANTHROPIC_KEY"
        "#;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, text).unwrap();
        let cfg = Config::load_from(&p).unwrap();
        assert_eq!(cfg.model.as_deref(), Some("claude-opus"));
        assert_eq!(cfg.default_agent.as_deref(), Some("plan"));
        let anthropic = cfg.providers.get("anthropic").unwrap();
        assert_eq!(
            anthropic.api_base.as_deref(),
            Some("https://custom.example.com")
        );
        assert_eq!(anthropic.api_key_env.as_deref(), Some("MY_ANTHROPIC_KEY"));
    }

    #[test]
    fn unknown_fields_ignored() {
        let text = r#"
            model = "x"
            future_field = "ignored"
        "#;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, text).unwrap();
        let cfg = Config::load_from(&p).unwrap();
        assert_eq!(cfg.model.as_deref(), Some("x"));
    }
}
