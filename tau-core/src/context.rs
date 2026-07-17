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
            },
        }
    }

    pub fn epoch(&self) -> &ContextEpoch {
        &self.epoch
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
        let previous = std::mem::replace(
            &mut self.epoch,
            ContextEpoch {
                number: next_number,
                messages: Vec::new(),
                plan_context,
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
