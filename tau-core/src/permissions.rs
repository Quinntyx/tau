//! Ordered, persisted-friendly permission rules for tool calls.

use globset::{Glob, GlobMatcher};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Ask,
    #[serde(rename = "reject")]
    Reject,
    #[serde(rename = "allow-always")]
    AllowAlways,
    #[serde(rename = "reject-always")]
    RejectAlways,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub pattern: String,
    pub decision: Decision,
    #[serde(skip)]
    matcher: Option<GlobMatcher>,
}

impl Rule {
    pub fn new(pattern: impl Into<String>, decision: Decision) -> Result<Self, globset::Error> {
        let pattern = pattern.into();
        let matcher = Glob::new(&pattern)?.compile_matcher();
        Ok(Self {
            pattern,
            decision,
            matcher: Some(matcher),
        })
    }

    fn matches(&mut self, subject: &str) -> bool {
        if self.matcher.is_none() {
            self.matcher = Glob::new(&self.pattern)
                .ok()
                .map(|glob| glob.compile_matcher());
        }
        self.matcher
            .as_ref()
            .is_some_and(|matcher| matcher.is_match(subject))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionEngine {
    rules: Vec<Rule>,
    default: Decision,
    #[serde(default)]
    hard_denials: Vec<String>,
    #[serde(default = "default_network_decision")]
    network_default: Decision,
}

fn default_network_decision() -> Decision {
    Decision::Allow
}

impl Default for PermissionEngine {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            default: Decision::Ask,
            hard_denials: vec![
                "credential:*".into(),
                "system:*".into(),
                "delete:*".into(),
                "chmod:*".into(),
                "git:push*".into(),
            ],
            network_default: Decision::Allow,
        }
    }
}

impl PermissionEngine {
    pub fn with_default(mut self, decision: Decision) -> Self {
        self.default = decision;
        self
    }

    pub fn add_rule(&mut self, rule: Rule) {
        self.rules.push(rule);
    }

    /// The risk-based default is overridden for network operations.
    pub fn with_network_default(mut self) -> Self {
        self.network_default = Decision::Allow;
        self
    }

    pub fn with_network_decision(mut self, decision: Decision) -> Self {
        self.network_default = decision;
        self
    }

    pub fn add_hard_denial(&mut self, pattern: impl Into<String>) -> Result<(), globset::Error> {
        let pattern = pattern.into();
        Glob::new(&pattern)?;
        self.hard_denials.push(pattern);
        Ok(())
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    pub fn evaluate(&mut self, tool: &str, args: &serde_json::Value) -> Decision {
        if tool == "network" || tool.starts_with("network:") {
            return self.network_default;
        }
        let subject = format!("{tool}:{args}");
        if self.hard_denials.iter().any(|pattern| {
            Glob::new(pattern)
                .map(|g| g.compile_matcher().is_match(&subject))
                .unwrap_or(true)
        }) {
            return Decision::RejectAlways;
        }
        self.rules
            .iter_mut()
            .find_map(|rule| rule.matches(&subject).then_some(rule.decision))
            .unwrap_or(self.default)
    }
}

/// A durable-shaped approval request. The broker intentionally waits until a
/// reply arrives; dropping a client must not turn an approval into execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PermissionRequest {
    pub id: String,
    pub run_id: String,
    pub tool: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PermissionReply {
    Allow,
    Reject,
}

type PendingPermission = (PermissionRequest, Option<PermissionReply>);

#[derive(Clone, Default)]
pub struct PermissionBroker {
    pending: Arc<Mutex<std::collections::BTreeMap<String, PendingPermission>>>,
    changed: Arc<Notify>,
}

impl PermissionBroker {
    pub async fn request(
        &self,
        run_id: impl Into<String>,
        tool: impl Into<String>,
        arguments: serde_json::Value,
    ) -> PermissionRequest {
        let request = PermissionRequest {
            id: Uuid::new_v4().to_string(),
            run_id: run_id.into(),
            tool: tool.into(),
            arguments,
        };
        self.pending
            .lock()
            .await
            .insert(request.id.clone(), (request.clone(), None));
        request
    }

    pub async fn wait(&self, id: &str) -> Option<PermissionReply> {
        loop {
            let mut pending = self.pending.lock().await;
            if let Some((_, Some(reply))) = pending.get(id) {
                let reply = *reply;
                pending.remove(id);
                return Some(reply);
            }
            if !pending.contains_key(id) {
                return None;
            }
            drop(pending);
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    }

    pub async fn reply(&self, id: &str, reply: PermissionReply) -> bool {
        let mut pending = self.pending.lock().await;
        let Some((_, slot)) = pending.get_mut(id) else {
            return false;
        };
        *slot = Some(reply);
        self.changed.notify_one();
        true
    }

    pub async fn pending(&self) -> Vec<PermissionRequest> {
        self.pending
            .lock()
            .await
            .values()
            .map(|(request, _)| request.clone())
            .filter(|r| !r.run_id.is_empty())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionError {
    Ask { tool: String },
    Rejected { tool: String },
}

impl std::fmt::Display for PermissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ask { tool } => write!(f, "approval required for tool `{tool}`"),
            Self::Rejected { tool } => write!(f, "permission rejected for tool `{tool}`"),
        }
    }
}

impl std::error::Error for PermissionError {}

pub fn authorize(
    engine: &mut PermissionEngine,
    tool: &str,
    args: &serde_json::Value,
) -> Result<(), PermissionError> {
    match engine.evaluate(tool, args) {
        Decision::Allow | Decision::AllowAlways => Ok(()),
        Decision::Ask => Err(PermissionError::Ask {
            tool: tool.to_string(),
        }),
        Decision::Reject | Decision::RejectAlways => Err(PermissionError::Rejected {
            tool: tool.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_matching_rule_wins() {
        let mut permissions = PermissionEngine::default();
        permissions.add_rule(Rule::new("read:*", Decision::Allow).unwrap());
        permissions.add_rule(Rule::new("*", Decision::Reject).unwrap());
        assert_eq!(
            permissions.evaluate("read", &serde_json::json!({"path":"a.rs"})),
            Decision::Allow
        );
        assert_eq!(
            permissions.evaluate("write", &serde_json::json!({"path":"a.rs"})),
            Decision::Reject
        );
    }

    #[test]
    fn rules_round_trip_without_runtime_matcher() {
        let mut permissions = PermissionEngine::default();
        permissions.add_rule(Rule::new("bash:*", Decision::Ask).unwrap());
        let json = serde_json::to_string(&permissions).unwrap();
        let mut restored: PermissionEngine = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored.evaluate("bash", &serde_json::json!({"command":"ls"})),
            Decision::Ask
        );
    }
}
