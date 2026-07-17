//! Primary agent profiles and skill discovery.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    pub description: Option<String>,
    pub allowed_tools: Vec<String>,
    pub source: SkillSource,
    pub trusted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillSource {
    Global,
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentProfile {
    pub name: String,
    pub prompt: String,
    pub model: Option<String>,
    pub primary: bool,
    pub compaction_agent: Option<String>,
    pub cycle: bool,
}

#[derive(Debug, Clone, Default)]
pub struct AgentRegistry {
    profiles: BTreeMap<String, AgentProfile>,
    active: String,
}

impl AgentRegistry {
    pub fn builtins(model: Option<String>) -> Self {
        let compact = format!(
            "{}/compaction",
            model.clone().unwrap_or_else(|| "default".into())
        );
        let mut profiles = BTreeMap::new();
        profiles.insert(
            "plan".into(),
            AgentProfile {
                name: "plan".into(),
                prompt: AgentKind::Plan.system_instruction().into(),
                model: model.clone(),
                primary: true,
                compaction_agent: Some(compact.clone()),
                cycle: true,
            },
        );
        profiles.insert(
            "build".into(),
            AgentProfile {
                name: "build".into(),
                prompt: AgentKind::Code.system_instruction().into(),
                model,
                primary: true,
                compaction_agent: Some(compact),
                cycle: true,
            },
        );
        Self {
            profiles,
            active: "build".into(),
        }
    }
    pub fn profiles(&self) -> impl Iterator<Item = &AgentProfile> {
        self.profiles.values()
    }
    pub fn active(&self) -> Option<&AgentProfile> {
        self.profiles.get(&self.active)
    }
    pub fn switch(&mut self, name: &str) -> Result<&AgentProfile, String> {
        if !self.profiles.contains_key(name) {
            return Err(format!("unknown agent: {name}"));
        }
        self.active = name.into();
        Ok(self.profiles.get(name).expect("checked"))
    }
    pub fn validate(&self) -> Result<(), String> {
        for profile in self.profiles.values().filter(|p| p.primary) {
            let Some(compaction) = &profile.compaction_agent else {
                return Err(format!(
                    "primary agent {} must declare a compaction agent",
                    profile.name
                ));
            };
            if compaction.is_empty() {
                return Err(format!("empty compaction agent for {}", profile.name));
            }
        }
        Ok(())
    }
}

pub fn discover_skills(roots: &[PathBuf]) -> std::io::Result<Vec<Skill>> {
    let mut skills = Vec::new();
    for root in roots {
        let skill_root = root.join(".tau").join("skills");
        if !skill_root.is_dir() {
            continue;
        }
        let source = if roots.first() == Some(root) {
            SkillSource::Global
        } else {
            SkillSource::Local
        };
        for entry in walk_skill_files(&skill_root)? {
            let path = entry;
            let name = path
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("skill")
                .to_string();
            let name = if name.eq_ignore_ascii_case("skill") {
                path.parent()
                    .and_then(Path::file_name)
                    .and_then(|n| n.to_str())
                    .unwrap_or("skill")
                    .to_owned()
            } else {
                name
            };
            let text = std::fs::read_to_string(&path)?;
            let (metadata, instructions) = parse_skill(&text);
            skills.push(Skill {
                name,
                description: metadata.get("description").cloned(),
                allowed_tools: metadata
                    .get("allowed-tools")
                    .map(|v| v.split_whitespace().map(str::to_owned).collect())
                    .unwrap_or_default(),
                instructions,
                path,
                source,
                trusted: matches!(source, SkillSource::Global),
            });
        }
    }
    skills.sort_by(|left, right| {
        (
            left.name.clone(),
            !matches!(left.source, SkillSource::Global),
        )
            .cmp(&(
                right.name.clone(),
                !matches!(right.source, SkillSource::Global),
            ))
    });
    skills.dedup_by(|left, right| {
        if left.name == right.name {
            right.trusted = left.trusted;
            true
        } else {
            false
        }
    });
    Ok(skills)
}

/// Global skills win over cwd-local skills of the same name. Local skills are
/// untrusted unless explicitly approved by name.
pub fn discover_skills_with_precedence(
    global_root: &Path,
    local_root: &Path,
    trusted_local: &[String],
) -> std::io::Result<Vec<Skill>> {
    let mut skills = discover_skills(&[global_root.to_path_buf(), local_root.to_path_buf()])?;
    for skill in &mut skills {
        if matches!(skill.source, SkillSource::Local) {
            skill.trusted = trusted_local.iter().any(|name| name == &skill.name);
        }
    }
    Ok(skills)
}

fn walk_skill_files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    if !root.is_dir() {
        return Ok(result);
    }
    for entry in std::fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            if path.join("SKILL.md").is_file() {
                result.push(path.join("SKILL.md"));
            } else {
                result.extend(walk_skill_files(&path)?);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            result.push(path);
        }
    }
    Ok(result)
}

fn parse_skill(text: &str) -> (BTreeMap<String, String>, String) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return (BTreeMap::new(), text.to_owned());
    };
    let Some((front, body)) = rest.split_once("\n---\n") else {
        return (BTreeMap::new(), text.to_owned());
    };
    let metadata = front
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().trim_matches('"').to_owned()))
        .collect();
    (metadata, body.to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpansionFailure {
    pub command: String,
    pub message: String,
}

/// Expand Claude's dynamic context syntax exactly once. Commands run with no
/// recursion and only conservative read-only classifications are accepted.
pub fn expand_dynamic_context(
    input: &str,
    cwd: &Path,
    arguments: &str,
    max_bytes: usize,
) -> Result<String, ExpansionFailure> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("!`") {
        out.push_str(&rest[..start]);
        let command_start = start + 2;
        let Some(end_rel) = rest[command_start..].find('`') else {
            return Err(ExpansionFailure {
                command: rest[command_start..].into(),
                message: "unterminated dynamic command".into(),
            });
        };
        let end = command_start + end_rel;
        let command = rest[command_start..end].replace("$ARGUMENTS", arguments);
        if !matches!(
            crate::tools::classify_command(&command),
            crate::tools::CommandClass::ReadOnly
        ) {
            return Err(ExpansionFailure {
                command,
                message: "dynamic command is not read-only".into(),
            });
        }
        let output = Command::new("sh")
            .arg("-c")
            .arg(&command)
            .current_dir(cwd)
            .output()
            .map_err(|e| ExpansionFailure {
                command: command.clone(),
                message: e.to_string(),
            })?;
        if !output.status.success() {
            return Err(ExpansionFailure {
                command,
                message: String::from_utf8_lossy(&output.stderr).trim().into(),
            });
        }
        let value = String::from_utf8_lossy(&output.stdout);
        if value.len() > max_bytes {
            return Err(ExpansionFailure {
                command,
                message: "dynamic command output exceeds limit".into(),
            });
        }
        out.push_str(value.trim_end_matches(['\r', '\n']));
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

pub fn skill_context(skills: &[Skill]) -> String {
    skills
        .iter()
        .map(|skill| format!("## Skill: {}\n{}", skill.name, skill.instructions))
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub fn project_root(path: impl AsRef<Path>) -> PathBuf {
    detect_cwd_root(path, false)
}

pub fn detect_cwd_root(path: impl AsRef<Path>, grouping: bool) -> PathBuf {
    let path = std::fs::canonicalize(path.as_ref()).unwrap_or_else(|_| path.as_ref().to_path_buf());
    if !grouping {
        return path;
    }
    path.ancestors()
        .find(|p| p.join(".git").exists() || p.join(".tau").exists())
        .unwrap_or(path.as_path())
        .to_path_buf()
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

    #[test]
    fn dynamic_context_expands_once_and_rejects_mutation() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            expand_dynamic_context("before !`printf ok` after", root.path(), "", 100).unwrap(),
            "before ok after"
        );
        assert!(expand_dynamic_context("!`echo !`printf nested` `", root.path(), "", 100).is_ok());
        assert!(expand_dynamic_context("!`touch x`", root.path(), "", 100).is_err());
    }
}
