//! tau configuration (global, MVP).
//!
//! Minimal schema grown per-milestone. File: `~/.config/tau/config.kdl`. All
//! fields optional; unknown fields are rejected so configuration mistakes fail
//! closed. Credentials are intentionally not part of this file.
//! per-project `.tau/` cascade is deferred with the project system.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::permissions::{Decision, PermissionEngine, Rule};
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
    /// Ordered global permission rules. Session rules are persisted separately.
    pub permissions: PermissionEngine,
    /// Named primary and compaction agent definitions.
    pub agents: BTreeMap<String, AgentConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    pub prompt: Option<String>,
    pub model: Option<String>,
    pub primary: bool,
    pub compaction_agent: Option<String>,
    pub cycle: bool,
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

/// Default config file path: `$XDG_CONFIG_HOME/tau/config.kdl`.
pub fn config_path() -> Result<PathBuf> {
    let base = dirs::config_dir().context("could not determine config directory")?;
    Ok(base.join("tau").join("config.kdl"))
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        if let Some(default) = &self.default_agent {
            if !self.agents.is_empty() && !self.agents.contains_key(default) {
                anyhow::bail!("default agent {default} is not configured");
            }
        }
        for (name, agent) in &self.agents {
            if agent.primary
                && agent
                    .compaction_agent
                    .as_deref()
                    .unwrap_or_default()
                    .is_empty()
            {
                anyhow::bail!("primary agent {name} must declare compaction_agent");
            }
        }
        Ok(())
    }
    /// Load config from the default path. Missing file ⇒ defaults.
    pub fn load() -> Result<Config> {
        let path = config_path()?;
        Self::load_from(&path)
    }

    /// Load config from an explicit path. Missing file ⇒ defaults.
    pub fn load_from(path: &Path) -> Result<Config> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                if path.extension().and_then(|s| s.to_str()) == Some("toml") {
                    anyhow::bail!(
                        "{} is TOML; tau now requires KDL (rename it to config.kdl and convert it)",
                        path.display()
                    );
                }
                Self::parse_kdl(&text).with_context(|| format!("parsing {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Parse and validate the public KDL configuration format.
    pub fn parse_kdl(text: &str) -> Result<Config> {
        let document: kdl::KdlDocument = text.parse().context("invalid KDL")?;
        let mut cfg = Config::default();
        let nodes = document.nodes();
        for node in nodes {
            let name = node.name().value();
            match name {
                "model" => cfg.model = Some(string_arg(node, "model")?),
                "default_agent" => cfg.default_agent = Some(string_arg(node, "default_agent")?),
                "provider" => {
                    let id = string_arg(node, "provider")?;
                    let children = node
                        .children()
                        .ok_or_else(|| anyhow::anyhow!("provider `{id}` requires a child block"))?;
                    let mut provider = ProviderConfig::default();
                    for child in children.nodes() {
                        match child.name().value() {
                            "api_base" => provider.api_base = Some(string_arg(child, "api_base")?),
                            "api_key_env" => {
                                provider.api_key_env = Some(string_arg(child, "api_key_env")?)
                            }
                            other => anyhow::bail!("unknown provider field `{other}`"),
                        }
                    }
                    cfg.providers.insert(id, provider);
                }
                "permission" => {
                    let pattern = string_arg(node, "permission")?;
                    let decision = property(node, "decision").unwrap_or("ask");
                    let decision = parse_decision(decision)?;
                    cfg.permissions.add_rule(Rule::new(pattern, decision)?);
                }
                "agent" => {
                    let id = string_arg(node, "agent")?;
                    let children = node
                        .children()
                        .ok_or_else(|| anyhow::anyhow!("agent `{id}` requires a child block"))?;
                    let mut agent = AgentConfig {
                        primary: bool_property(node, "primary").unwrap_or(false),
                        cycle: bool_property(node, "cycle").unwrap_or(false),
                        ..AgentConfig::default()
                    };
                    for child in children.nodes() {
                        match child.name().value() {
                            "prompt" => agent.prompt = Some(string_arg(child, "prompt")?),
                            "model" => agent.model = Some(string_arg(child, "model")?),
                            "compaction_agent" => {
                                agent.compaction_agent = Some(string_arg(child, "compaction_agent")?)
                            }
                            other => anyhow::bail!("unknown agent field `{other}`"),
                        }
                    }
                    cfg.agents.insert(id, agent);
                }
                other => anyhow::bail!("unknown KDL config node `{other}`"),
            }
        }
        Ok(cfg)
    }
}

fn string_arg(node: &kdl::KdlNode, what: &str) -> Result<String> {
    node.entries()
        .first()
        .and_then(|entry| entry.value().as_string())
        .map(str::to_owned)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{what} requires a quoted string argument (span {:?})",
                node.span()
            )
        })
}

fn property<'a>(node: &'a kdl::KdlNode, key: &str) -> Option<&'a str> {
    node.entries().iter().find_map(|entry| {
        entry
            .name()
            .is_some_and(|name| name.value() == key)
            .then(|| entry.value().as_string())
            .flatten()
    })
}

fn bool_property(node: &kdl::KdlNode, key: &str) -> Option<bool> {
    node.entries().iter().find_map(|entry| {
        entry
            .name()
            .is_some_and(|name| name.value() == key)
            .then(|| entry.value().as_bool())
            .flatten()
    })
}

fn parse_decision(value: &str) -> Result<Decision> {
    match value {
        "allow" => Ok(Decision::Allow),
        "ask" => Ok(Decision::Ask),
        "reject" | "deny" => Ok(Decision::Reject),
        "allow-always" => Ok(Decision::AllowAlways),
        "reject-always" => Ok(Decision::RejectAlways),
        _ => anyhow::bail!("invalid permission decision `{value}`"),
    }
}

#[cfg(test)]
mod tests;
