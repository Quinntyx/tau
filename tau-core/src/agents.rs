//! Primary agent profiles and skill discovery.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Plan,
    Code,
}

impl AgentKind {
    pub fn next(self) -> Self {
        match self {
            Self::Plan => Self::Code,
            Self::Code => Self::Plan,
        }
    }

    pub fn system_instruction(self) -> &'static str {
        match self {
            Self::Plan => {
                "You are the Plan agent. Read and reason first; do not mutate files until the current plan step is airtight."
            }
            Self::Code => {
                "You are the Code agent. Implement the current airtight plan step, using the available tools and reporting each mutation."
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub path: PathBuf,
    pub instructions: String,
}

pub fn discover_skills(roots: &[PathBuf]) -> std::io::Result<Vec<Skill>> {
    let mut skills = Vec::new();
    for root in roots {
        let skill_root = root.join(".tau").join("skills");
        if !skill_root.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(skill_root)? {
            let path = entry?.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
                continue;
            }
            let name = path
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("skill")
                .to_string();
            skills.push(Skill {
                name,
                instructions: std::fs::read_to_string(&path)?,
                path,
            });
        }
    }
    skills.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(skills)
}

pub fn skill_context(skills: &[Skill]) -> String {
    skills
        .iter()
        .map(|skill| format!("## Skill: {}\n{}", skill.name, skill.instructions))
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub fn project_root(path: impl AsRef<Path>) -> PathBuf {
    path.as_ref().to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_cycle() {
        assert_eq!(AgentKind::Plan.next(), AgentKind::Code);
        assert_eq!(AgentKind::Code.next(), AgentKind::Plan);
    }

    #[test]
    fn skills_are_discovered_in_stable_order() {
        let root = tempfile::tempdir().unwrap();
        let skills = root.path().join(".tau").join("skills");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::write(skills.join("z.md"), "z").unwrap();
        std::fs::write(skills.join("a.md"), "a").unwrap();
        let found = discover_skills(&[root.path().to_path_buf()]).unwrap();
        assert_eq!(
            found
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            ["a", "z"]
        );
    }
}
