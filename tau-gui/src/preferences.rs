//! GUI-only preferences. Deliberately separate from daemon/TUI configuration.
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GuiPreferences {
    pub favorites: Vec<String>,
    pub recent_models: Vec<String>,
    pub sidebar: bool,
    pub autonomy: bool,
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
                other => anyhow::bail!("unknown GUI preference node `{other}`"),
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
            out.push_str(&format!("favorite \"{value}\"\n"));
        }
        for value in &self.recent_models {
            out.push_str(&format!("recent \"{value}\"\n"));
        }
        out.push_str(&format!(
            "sidebar #{}\nautonomy #{}\n",
            self.sidebar, self.autonomy
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
        };
        x.save_to(&p).unwrap();
        assert_eq!(GuiPreferences::load_from(&p).unwrap(), x);
    }
}
