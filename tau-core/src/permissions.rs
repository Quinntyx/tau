//! Ordered, persisted-friendly permission rules for tool calls.

use globset::{Glob, GlobMatcher};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Ask,
    Deny,
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
}

impl Default for PermissionEngine {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            default: Decision::Ask,
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

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    pub fn evaluate(&mut self, tool: &str, args: &serde_json::Value) -> Decision {
        let subject = format!("{tool}:{args}");
        self.rules
            .iter_mut()
            .find_map(|rule| rule.matches(&subject).then_some(rule.decision))
            .unwrap_or(self.default)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionError {
    Ask { tool: String },
    Denied { tool: String },
}

impl std::fmt::Display for PermissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ask { tool } => write!(f, "approval required for tool `{tool}`"),
            Self::Denied { tool } => write!(f, "permission denied for tool `{tool}`"),
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
        Decision::Allow => Ok(()),
        Decision::Ask => Err(PermissionError::Ask {
            tool: tool.to_string(),
        }),
        Decision::Deny => Err(PermissionError::Denied {
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
        permissions.add_rule(Rule::new("*", Decision::Deny).unwrap());
        assert_eq!(
            permissions.evaluate("read", &serde_json::json!({"path":"a.rs"})),
            Decision::Allow
        );
        assert_eq!(
            permissions.evaluate("write", &serde_json::json!({"path":"a.rs"})),
            Decision::Deny
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
