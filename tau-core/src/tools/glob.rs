use std::path::PathBuf;

use globset::Glob;
use ignore::WalkBuilder;
use serde::Deserialize;

use super::error::{ToolError, io};
use super::registry::{Tool, ToolDescriptor};
use super::types::{GlobOutput, ToolContext};

#[derive(Debug, Clone, Deserialize)]
pub struct GlobInput {
    pub pattern: String,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct GlobTool;

impl Tool for GlobTool {
    type Input = GlobInput;
    type Output = GlobOutput;

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "glob".into(),
            description: "Find files by glob pattern with bounded deterministic results.".into(),
        }
    }

    fn execute(&self, input: GlobInput, context: &ToolContext) -> Result<GlobOutput, ToolError> {
        let requested = input.path.unwrap_or_else(|| PathBuf::from("."));
        let root = context
            .policy
            .resolve(&context.cwd, requested, "glob")?
            .path;
        if !std::fs::metadata(&root)
            .map_err(|e| io("stat", &root, e))?
            .is_dir()
        {
            return Err(ToolError::NotDirectory(root));
        }
        let matcher = Glob::new(&input.pattern)
            .map_err(|e| ToolError::InvalidPattern {
                pattern: input.pattern.clone(),
                message: e.to_string(),
            })?
            .compile_matcher();
        let mut entries = Vec::new();
        let limit = context.limits.glob_results;
        for item in WalkBuilder::new(&root)
            .hidden(true)
            .git_ignore(true)
            .git_global(false)
            .git_exclude(false)
            .parents(false)
            .follow_links(false)
            .build()
        {
            let item = item.map_err(|e| ToolError::Io {
                operation: "glob",
                path: root.clone(),
                source: std::io::Error::other(e.to_string()),
            })?;
            let path = item.path();
            if path == root || !item.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let relative = path.strip_prefix(&root).unwrap_or(path);
            if relative
                .components()
                .any(|component| component.as_os_str() == ".git")
            {
                continue;
            }
            let relative = normalize_path(relative);
            if matcher.is_match(&relative) {
                entries.push(PathBuf::from(relative));
                if entries.len() > limit {
                    break;
                }
            }
        }
        entries.sort();
        let truncated = entries.len() > limit;
        entries.truncate(limit);
        Ok(GlobOutput {
            root,
            entries,
            truncated,
        })
    }

    fn render(&self, output: &GlobOutput) -> String {
        let mut lines = if output.entries.is_empty() {
            vec!["No files found".into()]
        } else {
            output
                .entries
                .iter()
                .map(|path| path.display().to_string())
                .collect()
        };
        if output.truncated {
            lines.push("(Results truncated. Consider a narrower pattern.)".into());
        }
        lines.join("\n")
    }
}

fn normalize_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
