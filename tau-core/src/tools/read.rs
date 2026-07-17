use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::error::{ToolError, io};
use super::hashline;
use super::registry::{Tool, ToolDescriptor};
use super::types::{
    BinaryRead, DirectoryEntry, DirectoryRead, EntryKind, FileRead, ReadOutput, ToolContext,
};

#[derive(Debug, Clone, Deserialize)]
pub struct ReadInput {
    pub file_path: PathBuf,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct ReadTool;

impl Tool for ReadTool {
    type Input = ReadInput;
    type Output = ReadOutput;

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "read".into(),
            description: "Read a file or directory with bounded hashline output; directory IDs and refs are read-only until M5 edit support.".into(),
        }
    }

    fn execute(&self, input: ReadInput, context: &ToolContext) -> Result<ReadOutput, ToolError> {
        let resolved = context
            .policy
            .resolve(&context.cwd, &input.file_path, "read")?;
        let metadata =
            std::fs::metadata(&resolved.path).map_err(|e| io("stat", &resolved.path, e))?;
        if metadata.is_dir() {
            return read_directory(&resolved.path, &input, context);
        }
        if !metadata.is_file() {
            return Err(ToolError::NotFile(resolved.path));
        }
        read_file(&resolved.path, &input, context)
    }

    fn render(&self, output: &ReadOutput) -> String {
        match output {
            ReadOutput::File(value) => value.rendered.clone(),
            ReadOutput::Directory(value) => value.rendered.clone(),
            ReadOutput::Binary(value) => value.rendered.clone(),
        }
    }
}

fn read_file(
    path: &Path,
    input: &ReadInput,
    context: &ToolContext,
) -> Result<ReadOutput, ToolError> {
    let bytes = std::fs::read(path).map_err(|e| io("read", path, e))?;
    let mime = mime_guess::from_path(path)
        .first_or_octet_stream()
        .essence_str()
        .to_string();
    if is_binary(path, &bytes) {
        let truncated = bytes.len() > context.limits.binary_bytes;
        let bytes = bytes
            .into_iter()
            .take(context.limits.binary_bytes)
            .collect();
        return Ok(ReadOutput::Binary(BinaryRead {
            path: path.to_path_buf(),
            mime: mime.clone(),
            bytes,
            truncated,
            rendered: format!(
                "<path>{}</path>\n<type>binary</type>\n<mime>{mime}</mime>\n(binary content returned as typed bytes)\n",
                path.display()
            ),
        }));
    }
    let raw = String::from_utf8(bytes).map_err(|_| ToolError::BinaryFile {
        path: path.to_path_buf(),
        mime,
    })?;
    let offset = input.offset.unwrap_or(1);
    let limit = input.limit.unwrap_or(context.limits.read_lines);
    let rendered = hashline::render_file(
        &raw,
        path,
        offset,
        limit,
        context.limits.max_line_chars,
        context.limits.read_bytes,
    )
    .map_err(ToolError::InvalidInput)?;
    let parsed = hashline::parse_file(&raw);
    let start = offset.saturating_sub(1).min(parsed.lines.len());
    let end = (start + limit).min(parsed.lines.len());
    Ok(ReadOutput::File(FileRead {
        path: path.to_path_buf(),
        rev: rendered.rev,
        content: parsed.lines[start..end].join("\n"),
        line_start: rendered.line_start,
        line_end: rendered.line_end,
        total_lines: rendered.total_lines,
        truncated: rendered.truncated,
        rendered: rendered.content,
    }))
}

fn read_directory(
    path: &Path,
    input: &ReadInput,
    context: &ToolContext,
) -> Result<ReadOutput, ToolError> {
    let entries = directory_entries(path)?;
    let offset = input.offset.unwrap_or(1);
    let limit = input.limit.unwrap_or(context.limits.directory_entries);
    let rendered = hashline::render_directory(&entries, path, offset, limit);
    let start = offset.saturating_sub(1).min(entries.len());
    let end = (start + limit).min(entries.len());
    Ok(ReadOutput::Directory(DirectoryRead {
        path: path.to_path_buf(),
        rev: rendered.rev,
        entries: entries[start..end].to_vec(),
        offset,
        total_entries: entries.len(),
        truncated: rendered.truncated,
        rendered: rendered.content,
    }))
}

pub(crate) fn directory_entries(path: &Path) -> Result<Vec<DirectoryEntry>, ToolError> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(path).map_err(|e| io("list", path, e))? {
        let entry = entry.map_err(|e| io("list", path, e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let kind = entry_kind(&entry.path()).map_err(|e| io("stat", entry.path(), e))?;
        names.push((name, kind));
    }
    names.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(names
        .into_iter()
        .enumerate()
        .map(|(index, (name, kind))| DirectoryEntry {
            id: index + 1,
            name,
            kind,
        })
        .collect())
}

pub(crate) fn entry_kind(path: &Path) -> std::io::Result<EntryKind> {
    let kind = std::fs::symlink_metadata(path)?.file_type();
    if kind.is_symlink() {
        Ok(EntryKind::Symlink)
    } else if kind.is_dir() {
        Ok(EntryKind::Directory)
    } else {
        Ok(EntryKind::File)
    }
}

fn is_binary(path: &Path, bytes: &[u8]) -> bool {
    const EXTENSIONS: &[&str] = &[
        "zip", "tar", "gz", "exe", "dll", "so", "class", "jar", "war", "7z", "doc", "docx", "xls",
        "xlsx", "ppt", "pptx", "odt", "ods", "odp", "bin", "dat", "obj", "o", "a", "lib", "wasm",
        "pyc", "pyo",
    ];
    if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
    {
        return true;
    }
    if bytes.is_empty() {
        return false;
    }
    let non_printable = bytes
        .iter()
        .filter(|byte| **byte == 0 || (**byte < 9 || (**byte > 13 && **byte < 32)))
        .count();
    non_printable * 100 / bytes.len() > 30
}
