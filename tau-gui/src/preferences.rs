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
        let mut result = Self::default();
        for line in text.lines() {
            let mut p = line.splitn(2, ' ');
            let key = p.next().unwrap_or("");
            let value = p.next().unwrap_or("").trim().trim_matches('"').to_owned();
            match key {
                "favorite" => result.favorites.push(value),
                "recent" => result.recent_models.push(value),
                "sidebar" => result.sidebar = value == "true",
                "autonomy" => result.autonomy = value == "true",
                _ => {}
            }
        }
        Ok(result)
    }
    pub fn save_to(&self, file: &Path) -> Result<()> {
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = String::from("// tau GUI preferences\n");
        for value in &self.favorites {
            out.push_str(&format!("favorite \"{value}\"\n"));
        }
        for value in &self.recent_models {
            out.push_str(&format!("recent \"{value}\"\n"));
        }
        out.push_str(&format!(
            "sidebar {}\nautonomy {}\n",
            self.sidebar, self.autonomy
        ));
        std::fs::write(file, out).context("writing gui.kdl")
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
