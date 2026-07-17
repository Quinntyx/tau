//! Context epochs and deterministic compaction.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEpoch {
    pub number: u32,
    pub messages: Vec<ContextMessage>,
    pub plan_context: Option<String>,
    /// The complete active state that must survive an epoch boundary.
    pub active_state: Option<String>,
    pub provider: Option<String>,
    pub compaction_model: Option<String>,
    pub retry_marker: bool,
    pub estimated_tokens: usize,
}

#[derive(Debug, Clone)]
pub struct ContextAssembler {
    limit: usize,
    epoch: ContextEpoch,
    /// Epochs are retained in creation order; callers may persist this as an
    /// append-only log rather than having to reconstruct replaced state.
    epochs: Vec<ContextEpoch>,
}

impl ContextAssembler {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            epoch: ContextEpoch {
                number: 0,
                messages: Vec::new(),
                plan_context: None,
                active_state: None,
                provider: None,
                compaction_model: None,
                retry_marker: false,
                estimated_tokens: 0,
            },
            epochs: Vec::new(),
        }
    }

    pub fn epoch(&self) -> &ContextEpoch {
        &self.epoch
    }

    /// Return the provider-ready messages for the current epoch.
    pub fn messages(&self) -> &[ContextMessage] {
        &self.epoch.messages
    }

    /// Compact the current epoch without losing the active plan or provider metadata.
    /// The caller supplies the compaction agent's summary.
    pub fn compact_with_summary(&mut self, summary: impl Into<String>) -> ContextEpoch {
        self.compact(summary)
    }

    pub fn push(&mut self, role: impl Into<String>, content: impl Into<String>) -> bool {
        self.epoch.messages.push(ContextMessage {
            role: role.into(),
            content: content.into(),
        });
        self.estimated_tokens() > self.limit
    }

    pub fn set_plan_context(&mut self, markdown: impl Into<String>) {
        self.epoch.plan_context = Some(markdown.into());
    }

    pub fn set_active_state(&mut self, state: impl Into<String>) {
        self.epoch.active_state = Some(state.into());
    }

    pub fn epochs(&self) -> &[ContextEpoch] {
        &self.epochs
    }

    pub fn set_provider_metadata(
        &mut self,
        provider: impl Into<String>,
        compaction_model: impl Into<String>,
    ) {
        self.epoch.provider = Some(provider.into());
        self.epoch.compaction_model = Some(compaction_model.into());
    }

    pub fn should_compact(&self) -> bool {
        self.estimated_tokens() > self.limit
    }
    pub fn automatic_threshold(compaction_limit: usize) -> usize {
        compaction_limit.saturating_mul(80) / 100
    }
    pub fn should_compact_at(&self, compaction_limit: usize) -> bool {
        self.estimated_tokens() >= Self::automatic_threshold(compaction_limit)
    }

    /// Whether the compaction agent is operating with a smaller window than
    /// the primary provider (useful for warning the user before compaction).
    pub fn has_smaller_window(primary_limit: usize, compaction_limit: usize) -> bool {
        compaction_limit < primary_limit
    }
    pub fn mark_overflow_retry(&mut self) -> bool {
        if self.epoch.retry_marker {
            false
        } else {
            self.epoch.retry_marker = true;
            true
        }
    }

    pub fn validate_selected_ranges(
        content: &str,
        ranges: &[(usize, usize)],
    ) -> Result<(), String> {
        let count = content.lines().count();
        if ranges
            .iter()
            .any(|(start, end)| *start == 0 || start > end || *end > count)
        {
            return Err("selected hashline range is outside the artifact".into());
        }
        if ranges.windows(2).any(|pair| pair[0].1 >= pair[1].0) {
            return Err("selected hashline ranges must be ordered and non-overlapping".into());
        }
        Ok(())
    }

    pub fn estimated_tokens(&self) -> usize {
        let messages = self
            .epoch
            .messages
            .iter()
            .map(|message| message.role.len() + message.content.len())
            .sum::<usize>();
        let plan = self.epoch.plan_context.as_ref().map_or(0, String::len);
        (messages + plan) / 4
    }

    pub fn compact(&mut self, summary: impl Into<String>) -> ContextEpoch {
        let next_number = self.epoch.number + 1;
        let estimated_tokens = self.estimated_tokens();
        let plan_context = self.epoch.plan_context.clone();
        let active_state = self.epoch.active_state.clone();
        let provider = self.epoch.provider.clone();
        let compaction_model = self.epoch.compaction_model.clone();
        let mut previous = std::mem::replace(
            &mut self.epoch,
            ContextEpoch {
                number: next_number,
                messages: Vec::new(),
                plan_context,
                active_state: active_state.clone(),
                provider,
                compaction_model,
                retry_marker: false,
                estimated_tokens: 0,
            },
        );
        previous.estimated_tokens = estimated_tokens;
        self.epochs.push(previous.clone());
        self.epoch.messages.push(ContextMessage {
            role: "system".into(),
            content: format!("Conversation summary:\n{}", summary.into()),
        });
        self.epoch.estimated_tokens = self.estimated_tokens();
        if let Some(state) = active_state {
            self.epoch.messages.push(ContextMessage {
                role: "system".into(),
                content: format!("Active state (reinject unchanged):\n{state}"),
            });
        }
        if let Some(plan) = self.epoch.plan_context.clone() {
            self.epoch.messages.push(ContextMessage {
                role: "system".into(),
                content: format!("Active plan (reinject unchanged):\n{plan}"),
            });
        }
        previous
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compaction_increments_epoch_and_reinjects_plan() {
        let mut context = ContextAssembler::new(1);
        context.set_plan_context("# Plan");
        context.push("user", "a long message");
        let old = context.compact("summary");
        assert_eq!(old.number, 0);
        assert_eq!(context.epoch().number, 1);
        assert_eq!(context.epoch().plan_context.as_deref(), Some("# Plan"));
        assert!(
            context
                .messages()
                .iter()
                .any(|message| message.content.contains("Active plan"))
        );
        assert_eq!(context.epochs().len(), 1);
    }

    #[test]
    fn compaction_reinjects_active_state_and_retry_is_once_per_epoch() {
        let mut context = ContextAssembler::new(100);
        context.set_active_state("open tool call: 42");
        assert!(context.mark_overflow_retry());
        assert!(!context.mark_overflow_retry());
        context.compact("summary");
        assert!(context.mark_overflow_retry());
        assert!(
            context
                .messages()
                .iter()
                .any(|message| message.content.contains("42"))
        );
    }

    #[test]
    fn selected_ranges_must_be_valid_and_non_overlapping() {
        assert!(ContextAssembler::validate_selected_ranges("a\nb\nc", &[(1, 2), (3, 3)]).is_ok());
        assert!(ContextAssembler::validate_selected_ranges("a\nb\nc", &[(1, 2), (2, 3)]).is_err());
        assert!(ContextAssembler::has_smaller_window(100, 80));
    }
}
