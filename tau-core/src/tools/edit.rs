use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::error::{ToolError, io};
use super::hashline;
use super::read::directory_entries;
use super::registry::{Tool, ToolDescriptor};
use super::snapshot::SnapshotStore;
use super::types::{EntryKind, ToolContext};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditInput {
    #[serde(alias = "filePath")]
    pub path: PathBuf,
    pub content: Option<String>,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub start_ref: Option<String>,
    pub end_ref: Option<String>,
    pub file_rev: Option<String>,
    pub safe_reapply: Option<bool>,
    pub operations: Option<Vec<EditOperation>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditOperation {
    pub op: Option<String>,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub start_ref: Option<String>,
    pub end_ref: Option<String>,
    pub content: Option<String>,
    pub parent: Option<PathBuf>,
    pub name: Option<String>,
    pub kind: Option<EntryKind>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EditOutput {
    pub path: PathBuf,
    pub changed: usize,
    pub snapshot_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct EditTool;

impl Tool for EditTool {
    type Input = EditInput;
    type Output = EditOutput;

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "edit".into(),
            description: "Apply validated hashline edits to files or ID-addressed create/rename/delete-empty directory operations.".into(),
        }
    }

    fn execute(&self, input: EditInput, context: &ToolContext) -> Result<EditOutput, ToolError> {
        let resolved = context.policy.resolve(&context.cwd, &input.path, "edit")?;
        let path = resolved.path;
        let snapshot = SnapshotStore::for_cwd(&context.cwd);
        context.mutation.with_path_lock(&path, || {
            let capture = snapshot.capture_paths(std::slice::from_ref(&path))?;
            let metadata = std::fs::symlink_metadata(&path).map_err(|e| io("stat", &path, e))?;
            if metadata.is_dir() {
                edit_directory(&path, &input, context, &capture.id)
            } else if metadata.is_file() {
                edit_file(&path, &input, context, &capture.id)
            } else {
                Err(ToolError::InvalidEdit(format!(
                    "unsupported edit target: {}",
                    path.display()
                )))
            }
        })
    }

    fn render(&self, output: &EditOutput) -> String {
        format!(
            "Edit applied successfully to {} ({} operation(s), snapshot {}).",
            output.path.display(),
            output.changed,
            output.snapshot_id
        )
    }
}

fn edit_file(
    path: &Path,
    input: &EditInput,
    context: &ToolContext,
    snapshot_id: &str,
) -> Result<EditOutput, ToolError> {
    let original_bytes = std::fs::read(path).map_err(|e| io("read", path, e))?;
    let (bom, text_bytes) = if original_bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        (true, &original_bytes[3..])
    } else {
        (false, original_bytes.as_slice())
    };
    let text = String::from_utf8(text_bytes.to_vec()).map_err(|_| ToolError::BinaryFile {
        path: path.to_path_buf(),
        mime: "application/octet-stream".into(),
    })?;
    let parsed = hashline::parse_file(&text);
    let actual_rev = hashline::compute_file_rev(&text);
    if let Some(expected) = input.file_rev.as_deref() {
        if actual_rev != expected.to_ascii_uppercase() {
            return Err(ToolError::StaleRevision {
                path: path.to_path_buf(),
                expected: expected.to_ascii_uppercase(),
                actual: actual_rev,
            });
        }
    }
    let operations = file_operations(input)?;
    let hash_length = hashline::adaptive_hash_length(parsed.lines.len());
    let mut resolved = operations
        .iter()
        .map(|operation| {
            resolve_operation(
                operation,
                &parsed.lines,
                hash_length,
                input.safe_reapply.unwrap_or(false),
                path,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    resolved.sort_by_key(|operation| operation.start);
    if resolved.windows(2).any(|pair| pair[0].end >= pair[1].start) {
        return Err(ToolError::OverlappingEdit);
    }
    let mut lines = parsed.lines.clone();
    for operation in resolved.iter().rev() {
        lines.splice(
            operation.start..=operation.end,
            operation.replacement.clone(),
        );
    }
    let next_text = hashline::stringify_file(&parsed, &lines);
    let mut next_bytes = Vec::with_capacity(next_text.len() + usize::from(bom) * 3);
    if bom {
        next_bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    next_bytes.extend_from_slice(next_text.as_bytes());
    if next_bytes == original_bytes {
        return Err(ToolError::InvalidEdit("edit produces no changes".into()));
    }
    context
        .mutation
        .replace_if_unchanged(path, &original_bytes, &next_bytes)?;
    Ok(EditOutput {
        path: path.to_path_buf(),
        changed: resolved.len(),
        snapshot_id: snapshot_id.into(),
    })
}

fn edit_directory(
    path: &Path,
    input: &EditInput,
    context: &ToolContext,
    snapshot_id: &str,
) -> Result<EditOutput, ToolError> {
    let entries = directory_entries(path)?;
    let rendered = hashline::render_directory(&entries, path, 1, entries.len().max(1));
    if let Some(expected) = input.file_rev.as_deref() {
        if rendered.rev != expected.to_ascii_uppercase() {
            return Err(ToolError::StaleRevision {
                path: path.to_path_buf(),
                expected: expected.to_ascii_uppercase(),
                actual: rendered.rev,
            });
        }
    }
    let operations = input
        .operations
        .as_ref()
        .ok_or_else(|| ToolError::InvalidEdit("directory edits require operations[]".into()))?;
    if operations.is_empty() {
        return Err(ToolError::InvalidEdit(
            "operations[] must not be empty".into(),
        ));
    }
    let lines = hashline::directory_lines(&entries);
    let hash_length = hashline::adaptive_hash_length(entries.len());
    let mut changed = 0;
    for operation in operations {
        match operation.op.as_deref().unwrap_or("replace") {
            "create" => {
                create_entry(path, operation, context)?;
                changed += 1;
            }
            "rename" => {
                let index = resolve_directory_ref(
                    operation.reference.as_deref(),
                    &lines,
                    hash_length,
                    path,
                )?;
                let entry = &entries[index];
                let name = operation
                    .name
                    .as_deref()
                    .ok_or_else(|| ToolError::InvalidEdit("rename requires name".into()))?;
                validate_name(name)?;
                let source = path.join(&entry.name);
                let target = path.join(name);
                if target.exists() {
                    return Err(ToolError::AlreadyExists(target));
                }
                std::fs::rename(&source, &target).map_err(|e| io("rename", &source, e))?;
                changed += 1;
            }
            "delete" => {
                let index = resolve_directory_ref(
                    operation.reference.as_deref(),
                    &lines,
                    hash_length,
                    path,
                )?;
                let target = path.join(&entries[index].name);
                match entries[index].kind {
                    EntryKind::Directory => std::fs::remove_dir(&target).map_err(|error| {
                        if error.kind() == std::io::ErrorKind::DirectoryNotEmpty {
                            ToolError::DirectoryNotEmpty(target.clone())
                        } else {
                            io("delete directory", &target, error)
                        }
                    })?,
                    _ => std::fs::remove_file(&target).map_err(|e| io("delete", &target, e))?,
                }
                changed += 1;
            }
            other => {
                return Err(ToolError::InvalidEdit(format!(
                    "unsupported directory operation: {other}"
                )));
            }
        }
    }
    Ok(EditOutput {
        path: path.to_path_buf(),
        changed,
        snapshot_id: snapshot_id.into(),
    })
}

fn create_entry(
    path: &Path,
    operation: &EditOperation,
    context: &ToolContext,
) -> Result<(), ToolError> {
    let parent = operation.parent.as_deref().unwrap_or(Path::new("."));
    let parent = context.policy.resolve(path, parent, "edit")?.path;
    let name = operation
        .name
        .as_deref()
        .ok_or_else(|| ToolError::InvalidEdit("create requires name".into()))?;
    validate_name(name)?;
    let target = parent.join(name);
    if target.exists() {
        return Err(ToolError::AlreadyExists(target));
    }
    match operation.kind.clone().unwrap_or(EntryKind::File) {
        EntryKind::Directory => {
            std::fs::create_dir(&target).map_err(|e| io("create directory", &target, e))
        }
        EntryKind::File => {
            let content = operation.content.as_deref().unwrap_or("");
            std::fs::write(&target, content).map_err(|e| io("create file", &target, e))
        }
        EntryKind::Symlink => Err(ToolError::InvalidEdit(
            "creating symlinks is not supported".into(),
        )),
    }
}

#[derive(Debug, Clone)]
struct ResolvedOperation {
    start: usize,
    end: usize,
    replacement: Vec<String>,
}

fn file_operations(input: &EditInput) -> Result<Vec<EditOperation>, ToolError> {
    if let Some(operations) = &input.operations {
        if operations.is_empty() {
            return Err(ToolError::InvalidEdit(
                "operations[] must not be empty".into(),
            ));
        }
        return Ok(operations.clone());
    }
    if input.reference.is_some() || input.start_ref.is_some() || input.end_ref.is_some() {
        return Ok(vec![EditOperation {
            op: Some("replace".into()),
            reference: input.reference.clone(),
            start_ref: input.start_ref.clone(),
            end_ref: input.end_ref.clone(),
            content: input.content.clone(),
            parent: None,
            name: None,
            kind: None,
        }]);
    }
    Err(ToolError::InvalidEdit(
        "edit requires ref, start_ref/end_ref, or operations[]".into(),
    ))
}

fn resolve_operation(
    operation: &EditOperation,
    lines: &[String],
    hash_length: usize,
    safe_reapply: bool,
    path: &Path,
) -> Result<ResolvedOperation, ToolError> {
    let start_ref = operation
        .reference
        .as_deref()
        .or(operation.start_ref.as_deref());
    let start = resolve_ref(start_ref, lines, hash_length, safe_reapply, path)?;
    let end = if operation.reference.is_some() {
        start
    } else {
        resolve_ref(
            operation.end_ref.as_deref(),
            lines,
            hash_length,
            safe_reapply,
            path,
        )?
    };
    if start > end {
        return Err(ToolError::InvalidEdit(
            "start reference is after end reference".into(),
        ));
    }
    let replacement = hashline::replacement_lines(operation.content.as_deref().unwrap_or(""));
    Ok(ResolvedOperation {
        start,
        end,
        replacement,
    })
}

fn resolve_ref(
    reference: Option<&str>,
    lines: &[String],
    hash_length: usize,
    safe_reapply: bool,
    path: &Path,
) -> Result<usize, ToolError> {
    let reference =
        reference.ok_or_else(|| ToolError::InvalidEdit("missing hashline reference".into()))?;
    let parsed = hashline::parse_ref(reference).map_err(ToolError::InvalidEdit)?;
    let matches = |index: usize| {
        let line = &lines[index];
        hashline::line_hash(line, hash_length) == parsed.hash
            && parsed.anchor.as_deref().is_none_or(|anchor| {
                hashline::anchor_hash(
                    lines.get(index.wrapping_sub(1)).map(String::as_str),
                    line,
                    lines.get(index + 1).map(String::as_str),
                    hash_length,
                ) == anchor
            })
    };
    let direct = parsed
        .line
        .checked_sub(1)
        .filter(|index| *index < lines.len());
    if let Some(index) = direct.filter(|index| matches(*index)) {
        return Ok(index);
    }
    if safe_reapply {
        let candidates = lines
            .iter()
            .enumerate()
            .filter_map(|(index, _)| matches(index).then_some(index))
            .collect::<Vec<_>>();
        if candidates.len() == 1 {
            return Ok(candidates[0]);
        }
    }
    Err(ToolError::StaleReference {
        path: path.to_path_buf(),
        message: format!("reference {reference} no longer matches"),
    })
}

fn resolve_directory_ref(
    reference: Option<&str>,
    lines: &[String],
    hash_length: usize,
    path: &Path,
) -> Result<usize, ToolError> {
    let reference = reference
        .ok_or_else(|| ToolError::InvalidEdit("directory operation requires ref".into()))?;
    let parsed = hashline::parse_ref(reference).map_err(ToolError::InvalidEdit)?;
    let index = parsed
        .line
        .checked_sub(1)
        .filter(|index| *index < lines.len())
        .ok_or_else(|| ToolError::StaleReference {
            path: path.to_path_buf(),
            message: format!("reference {reference} is outside the directory"),
        })?;
    let line = &lines[index];
    if hashline::line_hash(line, hash_length) != parsed.hash {
        return Err(ToolError::StaleReference {
            path: path.to_path_buf(),
            message: format!("reference {reference} no longer matches"),
        });
    }
    Ok(index)
}

fn validate_name(name: &str) -> Result<(), ToolError> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(ToolError::InvalidEdit(format!(
            "invalid entry name: {name}"
        )));
    }
    Ok(())
}
