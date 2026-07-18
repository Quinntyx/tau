//! GUI-only preferences. Deliberately separate from daemon/TUI configuration.
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuiPreferences {
    pub favorites: Vec<String>,
    pub recent_models: Vec<String>,
    pub selected_model: Option<String>,
    pub selected_agent: Option<String>,
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
            selected_model: None,
            selected_agent: None,
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
    pub const MAX_RECENT_MODELS: usize = 8;

    /// Make preference collections safe to use even when they came from an
    /// older (or hand-edited) preferences file.  Ordering is intentional:
    /// favorites retain their first-seen order, while recents retain their
    /// newest-first order.
    pub fn normalize(&mut self) {
        deduplicate(&mut self.favorites);
        deduplicate(&mut self.recent_models);
        self.recent_models.truncate(Self::MAX_RECENT_MODELS);
    }

    pub fn load() -> Result<Self> {
        Self::load_from(&path()?)
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&path()?)
    }

    pub fn toggle_favorite(&mut self, model: impl Into<String>) {
        let model = model.into();
        if self.favorites.iter().any(|value| value == &model) {
            self.favorites.retain(|value| value != &model);
        } else {
            self.favorites.push(model);
        }
    }

    pub fn select_model(&mut self, model: impl Into<String>) {
        let model = model.into();
        self.selected_model = Some(model.clone());
        self.record_recent_model(model);
    }

    pub fn select_agent(&mut self, agent: impl Into<String>) {
        self.selected_agent = Some(agent.into());
    }

    pub fn record_recent_model(&mut self, model: impl Into<String>) {
        let model = model.into();
        self.recent_models.retain(|value| value != &model);
        self.recent_models.insert(0, model);
        self.recent_models.truncate(Self::MAX_RECENT_MODELS);
    }

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
                "selected_model" => {
                    result.selected_model =
                        Some(value().context("selected_model requires a string")?)
                }
                "selected_agent" => {
                    result.selected_agent =
                        Some(value().context("selected_agent requires a string")?)
                }
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
        result.normalize();
        Ok(result)
    }
    pub fn save_to(&self, file: &Path) -> Result<()> {
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = String::new();
        let mut normalized = self.clone();
        normalized.normalize();
        for value in &normalized.favorites {
            out.push_str(&format!(
                "favorite {}\n",
                kdl::KdlValue::String(value.clone())
            ));
        }
        for value in &normalized.recent_models {
            out.push_str(&format!(
                "recent {}\n",
                kdl::KdlValue::String(value.clone())
            ));
        }
        if let Some(value) = &normalized.selected_model {
            out.push_str(&format!(
                "selected_model {}\n",
                kdl::KdlValue::String(value.clone())
            ));
        }
        if let Some(value) = &normalized.selected_agent {
            out.push_str(&format!(
                "selected_agent {}\n",
                kdl::KdlValue::String(value.clone())
            ));
        }
        out.push_str(&format!(
            "sidebar #{}\nautonomy #{}\ndaemon_warning #{}\ndaemon_owned #{}\n",
            normalized.sidebar,
            normalized.autonomy,
            normalized.daemon_warning,
            normalized.daemon_owned
        ));
        let temporary = file.with_extension("kdl.tmp");
        std::fs::write(&temporary, out).context("writing temporary gui.kdl")?;
        std::fs::rename(&temporary, file).context("committing gui.kdl atomically")
    }
}

fn deduplicate(values: &mut Vec<String>) {
    let mut unique = Vec::with_capacity(values.len());
    for value in values.drain(..) {
        if !unique.iter().any(|existing| existing == &value) {
            unique.push(value);
        }
    }
    *values = unique;
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
            selected_model: Some("b".into()),
            selected_agent: Some("code".into()),
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
            selected_model: None,
            selected_agent: None,
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

    #[test]
    fn favorites_toggle_and_recents_are_unique_and_bounded() {
        let mut preferences = GuiPreferences::default();
        preferences.toggle_favorite("model");
        preferences.toggle_favorite("model");
        assert!(preferences.favorites.is_empty());
        for index in 0..=GuiPreferences::MAX_RECENT_MODELS {
            preferences.record_recent_model(format!("model-{index}"));
        }
        preferences.record_recent_model("model-3");
        assert_eq!(
            preferences.recent_models.len(),
            GuiPreferences::MAX_RECENT_MODELS
        );
        assert_eq!(preferences.recent_models[0], "model-3");
        assert_eq!(
            preferences
                .recent_models
                .iter()
                .filter(|v| *v == "model-3")
                .count(),
            1
        );
    }

    #[test]
    fn selecting_model_and_agent_updates_persisted_picker_state() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("gui.kdl");
        let mut preferences = GuiPreferences::default();
        preferences.select_model("provider/model");
        preferences.select_agent("builder");
        preferences.save_to(&p).unwrap();
        let loaded = GuiPreferences::load_from(&p).unwrap();
        assert_eq!(loaded.selected_model.as_deref(), Some("provider/model"));
        assert_eq!(loaded.selected_agent.as_deref(), Some("builder"));
        assert_eq!(loaded.recent_models, vec!["provider/model"]);
    }

    #[test]
    fn loading_normalizes_duplicate_favorites_and_recents_without_filtering_catalog() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("gui.kdl");
        let mut text = String::new();
        text.push_str("favorite \"server/model\"\nfavorite \"server/model\"\n");
        for index in 0..(GuiPreferences::MAX_RECENT_MODELS + 2) {
            text.push_str(&format!(
                "recent \"model-{index}\"\nrecent \"model-{index}\"\n"
            ));
        }
        text.push_str("selected_model \"server/model\"\nselected_agent \"remote-agent\"\n");
        std::fs::write(&p, text).unwrap();

        let loaded = GuiPreferences::load_from(&p).unwrap();
        assert_eq!(loaded.favorites, vec!["server/model"]);
        assert_eq!(
            loaded.recent_models.len(),
            GuiPreferences::MAX_RECENT_MODELS
        );
        assert_eq!(loaded.recent_models[0], "model-0");
        assert_eq!(loaded.selected_model.as_deref(), Some("server/model"));
        assert_eq!(loaded.selected_agent.as_deref(), Some("remote-agent"));
    }

    #[test]
    fn save_normalizes_legacy_duplicates_and_preserves_selection() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("gui.kdl");
        let mut preferences = GuiPreferences::default();
        preferences.favorites = vec!["a".into(), "a".into(), "b".into()];
        preferences.recent_models = vec!["x".into(), "x".into()];
        preferences.selected_model = Some("catalog-only-model".into());
        preferences.save_to(&p).unwrap();
        let loaded = GuiPreferences::load_from(&p).unwrap();
        assert_eq!(loaded.favorites, vec!["a", "b"]);
        assert_eq!(loaded.recent_models, vec!["x"]);
        assert_eq!(loaded.selected_model.as_deref(), Some("catalog-only-model"));
    }

    #[test]
    fn malformed_known_nodes_still_report_errors_but_unknown_nodes_are_ignored() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("gui.kdl");
        std::fs::write(&p, "future_preference (not understood)\nunknown #true\n").unwrap();
        assert!(GuiPreferences::load_from(&p).is_ok());
        std::fs::write(&p, "selected_model #true\n").unwrap();
        assert!(GuiPreferences::load_from(&p).is_err());
    }
}
