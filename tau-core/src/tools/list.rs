use std::path::PathBuf;

use serde::Deserialize;

use super::error::{ToolError, io};
use super::read::directory_entries;
use super::registry::{Tool, ToolDescriptor};
use super::types::{EntryKind, ListOutput, ToolContext};

#[derive(Debug, Clone, Deserialize)]
pub struct ListInput {
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct ListTool;

impl Tool for ListTool {
    type Input = ListInput;
    type Output = ListOutput;

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "list".into(),
            description: "List direct directory entries in deterministic order.".into(),
        }
    }

    fn execute(&self, input: ListInput, context: &ToolContext) -> Result<ListOutput, ToolError> {
        let requested = input.path.unwrap_or_else(|| PathBuf::from("."));
        let resolved = context.policy.resolve(&context.cwd, requested, "list")?;
        if !std::fs::metadata(&resolved.path)
            .map_err(|e| io("stat", &resolved.path, e))?
            .is_dir()
        {
            return Err(ToolError::NotDirectory(resolved.path));
        }
        let mut entries = directory_entries(&resolved.path)?;
        let truncated = entries.len() > context.limits.directory_entries;
        entries.truncate(context.limits.directory_entries);
        Ok(ListOutput {
            path: resolved.path,
            entries,
            truncated,
        })
    }

    fn render(&self, output: &ListOutput) -> String {
        let mut lines = vec![
            format!("<path>{}</path>", output.path.display()),
            "<entries>".into(),
        ];
        lines.extend(output.entries.iter().map(|entry| {
            let suffix = if entry.kind == EntryKind::Directory {
                "/"
            } else {
                ""
            };
            format!("{:02}|{}{}", entry.id, entry.name, suffix)
        }));
        if output.truncated {
            lines.push("(Results truncated. Use a narrower directory.)".into());
        }
        lines.push("</entries>".into());
        lines.join("\n")
    }
}
