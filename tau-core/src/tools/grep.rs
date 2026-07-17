use std::path::PathBuf;

use globset::Glob;
use ignore::WalkBuilder;
use regex::Regex;
use serde::Deserialize;

use super::error::{ToolError, io};
use super::registry::{Tool, ToolDescriptor};
use super::types::{GrepMatch, GrepOutput, Submatch, ToolContext};

#[derive(Debug, Clone, Deserialize)]
pub struct GrepInput {
    pub pattern: String,
    pub path: Option<PathBuf>,
    pub include: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct GrepTool;

impl Tool for GrepTool {
    type Input = GrepInput;
    type Output = GrepOutput;

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "grep".into(),
            description: "Search text files with a regex and bounded structured matches.".into(),
        }
    }

    fn execute(&self, input: GrepInput, context: &ToolContext) -> Result<GrepOutput, ToolError> {
        let requested = input.path.unwrap_or_else(|| PathBuf::from("."));
        let root = context
            .policy
            .resolve(&context.cwd, requested, "grep")?
            .path;
        if !std::fs::metadata(&root)
            .map_err(|e| io("stat", &root, e))?
            .is_dir()
        {
            return Err(ToolError::NotDirectory(root));
        }
        let regex = Regex::new(&input.pattern).map_err(|e| ToolError::InvalidPattern {
            pattern: input.pattern.clone(),
            message: e.to_string(),
        })?;
        let include = input
            .include
            .as_deref()
            .map(Glob::new)
            .transpose()
            .map_err(|e| ToolError::InvalidPattern {
                pattern: input.include.clone().unwrap_or_default(),
                message: e.to_string(),
            })?
            .map(|glob| glob.compile_matcher());
        let mut matches = Vec::new();
        for item in WalkBuilder::new(&root)
            .hidden(false)
            .git_ignore(true)
            .git_global(false)
            .git_exclude(false)
            .parents(false)
            .follow_links(false)
            .build()
        {
            let item = item.map_err(|e| ToolError::Io {
                operation: "grep",
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
            if include
                .as_ref()
                .is_some_and(|matcher| !matcher.is_match(normalize_path(relative)))
            {
                continue;
            }
            let bytes = std::fs::read(path).map_err(|e| io("grep", path, e))?;
            if bytes.contains(&0) {
                continue;
            }
            let Ok(text) = String::from_utf8(bytes) else {
                continue;
            };
            let mut line_offset = 0;
            for (line_number, line) in text.split_inclusive('\n').enumerate() {
                let line_text = line
                    .strip_suffix('\n')
                    .unwrap_or(line)
                    .strip_suffix('\r')
                    .unwrap_or(line);
                let submatches = regex
                    .find_iter(line_text)
                    .map(|found| Submatch {
                        text: found.as_str().into(),
                        start: found.start(),
                        end: found.end(),
                    })
                    .collect::<Vec<_>>();
                if !submatches.is_empty() {
                    matches.push(GrepMatch {
                        path: path.to_path_buf(),
                        line: line_number + 1,
                        offset: line_offset,
                        text: line_text.into(),
                        submatches,
                    });
                    if matches.len() > context.limits.search_matches {
                        break;
                    }
                }
                line_offset += line.len();
            }
            if matches.len() > context.limits.search_matches {
                break;
            }
        }
        matches.sort_by(|left, right| left.path.cmp(&right.path).then(left.line.cmp(&right.line)));
        let truncated = matches.len() > context.limits.search_matches;
        matches.truncate(context.limits.search_matches);
        Ok(GrepOutput { matches, truncated })
    }

    fn render(&self, output: &GrepOutput) -> String {
        if output.matches.is_empty() {
            return "No files found".into();
        }
        let mut lines = vec![format!(
            "Found {} matches{}",
            output.matches.len(),
            if output.truncated {
                " (more matches available)"
            } else {
                ""
            }
        )];
        let mut current = None;
        for item in &output.matches {
            if current.as_ref() != Some(&item.path) {
                if current.is_some() {
                    lines.push(String::new());
                }
                current = Some(item.path.clone());
                lines.push(format!("{}:", item.path.display()));
            }
            lines.push(format!("  Line {}: {}", item.line, item.text));
        }
        if output.truncated {
            lines.push(String::new());
            lines.push("(Results truncated. Consider a narrower pattern.)".into());
        }
        lines.join("\n")
    }
}

fn normalize_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
