//! Structured conversation plan and mutation gate.

use crate::db::QaRecord;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub title: String,
    pub contract: String,
    pub steps: Vec<PlanStep>,
    pub current_step: Option<usize>,
    #[serde(default)]
    pub airtight_revoked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub title: String,
    pub airtight: bool,
    pub items: Vec<PlanItem>,
    pub qa_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanItem {
    pub text: String,
    pub checked: bool,
}

impl Plan {
    pub fn new(title: impl Into<String>, contract: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            contract: contract.into(),
            steps: Vec::new(),
            current_step: None,
            airtight_revoked: false,
        }
    }

    pub fn add_step(&mut self, title: impl Into<String>) -> usize {
        self.steps.push(PlanStep {
            title: title.into(),
            airtight: false,
            items: Vec::new(),
            qa_ids: Vec::new(),
        });
        let index = self.steps.len() - 1;
        self.current_step.get_or_insert(index);
        index
    }

    pub fn add_item(&mut self, step: usize, text: impl Into<String>) -> bool {
        let Some(step) = self.steps.get_mut(step) else {
            return false;
        };
        step.items.push(PlanItem {
            text: text.into(),
            checked: false,
        });
        true
    }

    pub fn set_current_step(&mut self, step: usize) -> bool {
        if step < self.steps.len() {
            self.current_step = Some(step);
            true
        } else {
            false
        }
    }

    pub fn mark_item(&mut self, step: usize, item: usize, checked: bool) -> bool {
        let Some(item) = self
            .steps
            .get_mut(step)
            .and_then(|step| step.items.get_mut(item))
        else {
            return false;
        };
        item.checked = checked;
        true
    }

    pub fn airtight_step(&mut self, step: usize) -> bool {
        if self.airtight_revoked {
            return false;
        }
        let Some(step) = self.steps.get_mut(step) else {
            return false;
        };
        step.airtight = true;
        true
    }

    /// Revocation is material: it cannot be bypassed by autonomous mode.
    pub fn revoke_airtight(&mut self) {
        self.airtight_revoked = true;
        for step in &mut self.steps {
            step.airtight = false;
        }
    }

    pub fn current_is_airtight(&self) -> bool {
        self.current_step
            .and_then(|index| self.steps.get(index))
            .is_some_and(|step| step.airtight)
            && !self.airtight_revoked
    }

    pub fn render_markdown(&self) -> String {
        let mut output = format!("# Plan: {}\n\n{}\n", self.title, self.contract);
        for (index, step) in self.steps.iter().enumerate() {
            let marker = if step.airtight {
                "airtight"
            } else {
                "not airtight"
            };
            output.push_str(&format!(
                "\n## Step {}: {} ({marker})\n",
                index + 1,
                step.title
            ));
            for item in &step.items {
                output.push_str(&format!(
                    "- [{}] {}\n",
                    if item.checked { "x" } else { " " },
                    item.text
                ));
            }
            if !step.qa_ids.is_empty() {
                output.push_str(&format!("- Q&A: {}\n", step.qa_ids.join(", ")));
            }
        }
        output
    }

    /// Render a revision with resolved, auditable Q&A citations.
    pub fn render_markdown_with_qa(&self, records: &[QaRecord]) -> String {
        let mut output = self.render_markdown();
        for step in &self.steps {
            for id in &step.qa_ids {
                if let Some(qa) = records.iter().find(|record| &record.id == id) {
                    output.push_str(&format!(
                        "\n> Q&A {} — {}\n> {}\n",
                        qa.id, qa.question, qa.answer
                    ));
                }
            }
        }
        output
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateError {
    NoAirtightStep,
}

impl std::fmt::Display for GateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the current plan step must be airtight before mutating tools can run")
    }
}

impl std::error::Error for GateError {}

pub fn allows_tool(plan: &Plan, _autonomous: bool, tool: &str) -> Result<(), GateError> {
    let mutating = matches!(
        tool,
        "edit" | "write" | "bash" | "delete" | "rename" | "patch" | "apply_patch" | "mkdir" | "rm"
    );
    if mutating && !plan.current_is_airtight() {
        return Err(GateError::NoAirtightStep);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_gate_requires_airtight_current_step() {
        let mut plan = Plan::new("test", "contract");
        plan.add_step("implementation");
        assert_eq!(
            allows_tool(&plan, false, "write"),
            Err(GateError::NoAirtightStep)
        );
        assert_eq!(
            allows_tool(&plan, true, "write"),
            Err(GateError::NoAirtightStep)
        );
        plan.airtight_step(0);
        assert!(allows_tool(&plan, false, "write").is_ok());
    }

    #[test]
    fn markdown_contains_step_state_and_items() {
        let mut plan = Plan::new("test", "contract");
        let step = plan.add_step("one");
        plan.add_item(step, "do thing");
        let markdown = plan.render_markdown();
        assert!(markdown.contains("not airtight"));
        assert!(markdown.contains("[ ] do thing"));
    }
}
