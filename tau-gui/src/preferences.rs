//! GUI-only preferences. Deliberately separate from daemon/TUI configuration.
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuiPreferences {
    pub favorites: Vec<String>,
    pub recent_models: Vec<String>,
    pub sidebar: bool,
    pub autonomy: bool,
    pub daemon_warning: bool,
    pub daemon_owned: bool,
}

impl Default for GuiPreferences {
    fn default() -> Self {
        Self {
            favorites: Vec::new(),
            recent_models: Vec::new(),
            sidebar: true,
            autonomy: false,
            daemon_warning: true,
            daemon_owned: true,
        }
    }
}

pub fn path() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("config directory")?
        .join("tau")
        .join("gui.kdl"))
}

impl GuiPreferences {
    pub fn load_from(file: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(file) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.into()),
        };
        let document: kdl::KdlDocument = text.parse().context("invalid GUI preferences KDL")?;
        let mut result = Self::default();
        for node in document.nodes() {
            let value = || {
                node.entries()
                    .first()
                    .and_then(|e| e.value().as_string())
                    .map(str::to_owned)
            };
            match node.name().value() {
                "favorite" => result
                    .favorites
                    .push(value().context("favorite requires a string")?),
                "recent" => result
                    .recent_models
                    .push(value().context("recent requires a string")?),
                "sidebar" => {
                    result.sidebar = node
                        .entries()
                        .first()
                        .and_then(|e| e.value().as_bool())
                        .context("sidebar requires bool")?
                }
                "autonomy" => {
                    result.autonomy = node
                        .entries()
                        .first()
                        .and_then(|e| e.value().as_bool())
                        .context("autonomy requires bool")?
                }
                "daemon_warning" => {
                    result.daemon_warning = node
                        .entries()
                        .first()
                        .and_then(|e| e.value().as_bool())
                        .context("daemon_warning requires bool")?
                }
                "daemon_owned" => {
                    result.daemon_owned = node
                        .entries()
                        .first()
                        .and_then(|e| e.value().as_bool())
                        .context("daemon_owned requires bool")?
                }
                // Keep loading preferences when a newer GUI has added a key.
                // Preferences are intentionally forward-compatible: unknown
                // nodes cannot affect the values understood by this version.
                _ => {}
            }
        }
        Ok(result)
    }
    pub fn save_to(&self, file: &Path) -> Result<()> {
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = String::new();
        for value in &self.favorites {
            out.push_str(&format!(
                "favorite {}\n",
                kdl::KdlValue::String(value.clone())
            ));
        }
        for value in &self.recent_models {
            out.push_str(&format!(
                "recent {}\n",
                kdl::KdlValue::String(value.clone())
            ));
        }
        out.push_str(&format!(
            "sidebar #{}\nautonomy #{}\ndaemon_warning #{}\ndaemon_owned #{}\n",
            self.sidebar, self.autonomy, self.daemon_warning, self.daemon_owned
        ));
        let temporary = file.with_extension("kdl.tmp");
        std::fs::write(&temporary, out).context("writing temporary gui.kdl")?;
        std::fs::rename(&temporary, file).context("committing gui.kdl atomically")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("gui.kdl");
        let x = GuiPreferences {
            favorites: vec!["a".into()],
            recent_models: vec!["b".into()],
            sidebar: true,
            autonomy: false,
            daemon_warning: true,
            daemon_owned: true,
        };
        x.save_to(&p).unwrap();
        assert_eq!(GuiPreferences::load_from(&p).unwrap(), x);
    }

    #[test]
    fn round_trip_escapes_kdl_strings() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("nested").join("gui.kdl");
        let x = GuiPreferences {
            favorites: vec!["a\"\\\nb".into()],
            recent_models: vec![],
            sidebar: true,
            autonomy: true,
            daemon_warning: false,
            daemon_owned: false,
        };
        x.save_to(&p).unwrap();
        assert_eq!(GuiPreferences::load_from(&p).unwrap(), x);
    }

    #[test]
    fn missing_values_use_defaults_and_unknown_nodes_are_ignored() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("gui.kdl");
        std::fs::write(
            &p,
            "future_preference #true\nsidebar #false\nunknown \"value\"\n",
        )
        .unwrap();

        let preferences = GuiPreferences::load_from(&p).unwrap();
        assert!(!preferences.sidebar);
        assert_eq!(preferences.autonomy, GuiPreferences::default().autonomy);
        assert_eq!(
            preferences.daemon_warning,
            GuiPreferences::default().daemon_warning
        );
        assert_eq!(
            preferences.daemon_owned,
            GuiPreferences::default().daemon_owned
        );
    }

    #[test]
    fn warning_and_ownership_persist_independently() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("gui.kdl");
        let mut preferences = GuiPreferences {
            daemon_warning: false,
            ..GuiPreferences::default()
        };
        preferences.save_to(&p).unwrap();
        let loaded = GuiPreferences::load_from(&p).unwrap();
        assert!(!loaded.daemon_warning);
        assert!(loaded.daemon_owned);

        preferences.daemon_warning = true;
        preferences.daemon_owned = false;
        preferences.save_to(&p).unwrap();
        let loaded = GuiPreferences::load_from(&p).unwrap();
        assert!(loaded.daemon_warning);
        assert!(!loaded.daemon_owned);
    }
}
