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
    pub provider: Option<String>,
    pub compaction_model: Option<String>,
    pub retry_marker: bool,
}

#[derive(Debug, Clone)]
pub struct ContextAssembler {
    limit: usize,
    epoch: ContextEpoch,
}

impl ContextAssembler {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            epoch: ContextEpoch {
                number: 0,
                messages: Vec::new(),
                plan_context: None,
                provider: None,
                compaction_model: None,
                retry_marker: false,
            },
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
        let plan_context = self.epoch.plan_context.clone();
        let provider = self.epoch.provider.clone();
        let compaction_model = self.epoch.compaction_model.clone();
        let previous = std::mem::replace(
            &mut self.epoch,
            ContextEpoch {
                number: next_number,
                messages: Vec::new(),
                plan_context,
                provider,
                compaction_model,
                retry_marker: false,
            },
        );
        self.epoch.messages.push(ContextMessage {
            role: "system".into(),
            content: format!("Conversation summary:\n{}", summary.into()),
        });
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
    }
}
