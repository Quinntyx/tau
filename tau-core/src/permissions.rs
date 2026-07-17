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

/// Lifetime requested for a human or steering decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionScope {
    Once,
    Session,
    Global,
    AlwaysAllow,
    AlwaysReject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptActor {
    Human,
    SteeringAgent,
    SuperAgent,
}

impl Decision {
    pub fn from_scope(choice: PermissionReply, scope: PermissionScope) -> Self {
        match (choice, scope) {
            (PermissionReply::Allow, PermissionScope::AlwaysAllow) => Self::AllowAlways,
            (PermissionReply::Reject, PermissionScope::AlwaysReject) => Self::RejectAlways,
            (PermissionReply::Allow, _) => Self::Allow,
            (PermissionReply::Reject, _) => Self::Reject,
        }
    }
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

impl PermissionReply {
    pub fn decision(self, scope: PermissionScope) -> Decision {
        Decision::from_scope(self, scope)
    }
}

/// Super mode may answer informational/soft prompts, but it is never a
/// policy or plan authority. Airtight transitions require a human or the
/// explicitly authorized steering actor.
pub fn actor_may_answer_soft_prompt(actor: PromptActor) -> bool {
    matches!(
        actor,
        PromptActor::Human | PromptActor::SteeringAgent | PromptActor::SuperAgent
    )
}

pub fn actor_may_grant_airtight(actor: PromptActor) -> bool {
    matches!(actor, PromptActor::Human | PromptActor::SteeringAgent)
}

pub fn actor_may_apply_permission(actor: PromptActor) -> bool {
    matches!(actor, PromptActor::Human | PromptActor::SteeringAgent)
}

/// A prompt may be interrupted by a daemon restart, but never expires while
/// the daemon is alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptStatus {
    Pending,
    Resolved,
    Interrupted,
}

type PendingPermission = (PermissionRequest, Option<PermissionReply>, String);

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
            .insert(request.id.clone(), (request.clone(), None, String::new()));
        request
    }

    pub async fn wait(&self, id: &str) -> Option<PermissionReply> {
        loop {
            let notified = self.changed.notified();
            let mut pending = self.pending.lock().await;
            if let Some((_, Some(reply), _)) = pending.get(id) {
                let reply = *reply;
                pending.remove(id);
                return Some(reply);
            }
            if !pending.contains_key(id) {
                return None;
            }
            drop(pending);
            notified.await;
        }
    }

    pub async fn reply(&self, id: &str, reply: PermissionReply) -> bool {
        let mut pending = self.pending.lock().await;
        let Some((_, slot, _)) = pending.get_mut(id) else {
            return false;
        };
        if slot.is_some() {
            return false;
        }
        *slot = Some(reply);
        self.changed.notify_one();
        true
    }

    pub async fn request_for_client(
        &self,
        client_id: impl Into<String>,
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
        let owner = client_id.into();
        self.pending
            .lock()
            .await
            .insert(request.id.clone(), (request.clone(), None, owner));
        request
    }

    pub async fn reply_owned(&self, id: &str, client_id: &str, reply: PermissionReply) -> bool {
        let mut pending = self.pending.lock().await;
        let Some((_, slot, owner)) = pending.get_mut(id) else {
            return false;
        };
        if (!owner.is_empty() && owner != client_id) || slot.is_some() {
            return false;
        }
        *slot = Some(reply);
        self.changed.notify_one();
        true
    }

    pub async fn take_over(&self, id: &str, client_id: impl Into<String>) -> bool {
        let mut pending = self.pending.lock().await;
        let Some((_, slot, owner)) = pending.get_mut(id) else {
            return false;
        };
        if slot.is_some() {
            return false;
        }
        *owner = client_id.into();
        true
    }

    /// Mark all unresolved prompts as interrupted during daemon shutdown or
    /// restart. Waiters observe removal and stop rather than executing stale
    /// work; resolved prompts remain available for their owners to consume.
    pub async fn interrupt_pending(&self) -> usize {
        let mut pending = self.pending.lock().await;
        let ids: Vec<_> = pending
            .iter()
            .filter_map(|(id, (_, reply, _))| reply.is_none().then_some(id.clone()))
            .collect();
        let count = ids.len();
        for id in ids {
            pending.remove(&id);
        }
        if count != 0 {
            self.changed.notify_waiters();
        }
        count
    }

    pub async fn pending(&self) -> Vec<PermissionRequest> {
        self.pending
            .lock()
            .await
            .values()
            .map(|(request, _, _)| request.clone())
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

    #[test]
    fn prompt_authority_never_lets_super_agent_change_policy() {
        assert!(actor_may_answer_soft_prompt(PromptActor::SuperAgent));
        assert!(!actor_may_apply_permission(PromptActor::SuperAgent));
        assert!(!actor_may_grant_airtight(PromptActor::SuperAgent));
        assert!(actor_may_apply_permission(PromptActor::Human));
    }

    #[tokio::test]
    async fn permission_broker_enforces_owner_and_interrupts_waiters() {
        let broker = PermissionBroker::default();
        let request = broker
            .request_for_client("owner", "run", "write", serde_json::json!({}))
            .await;
        assert!(
            !broker
                .reply_owned(&request.id, "other", PermissionReply::Allow)
                .await
        );
        assert!(
            broker
                .reply_owned(&request.id, "owner", PermissionReply::Allow)
                .await
        );
        assert_eq!(broker.wait(&request.id).await, Some(PermissionReply::Allow));

        let pending = broker
            .request_for_client("owner", "run", "write", serde_json::json!({}))
            .await;
        let waiter = {
            let broker = broker.clone();
            let id = pending.id.clone();
            tokio::spawn(async move { broker.wait(&id).await })
        };
        assert_eq!(broker.interrupt_pending().await, 1);
        assert_eq!(waiter.await.unwrap(), None);
    }
}
