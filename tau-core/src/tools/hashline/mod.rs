//! Hashline references used by read output and the later edit tool.

use sha1::{Digest, Sha1};

use super::types::DirectoryEntry;

const HASH_THRESHOLD: usize = 4_096;
const SMALL_HASH: usize = 3;
const LARGE_HASH: usize = 4;
const REV_LENGTH: usize = 8;
const SEPARATOR: char = '\u{241e}';

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFile {
    pub lines: Vec<String>,
    pub eol: &'static str,
    pub ends_with_newline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRef {
    pub line: usize,
    pub hash: String,
    pub anchor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RenderedFile {
    pub rev: String,
    pub content: String,
    pub line_start: usize,
    pub line_end: usize,
    pub total_lines: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct RenderedDirectory {
    pub rev: String,
    pub content: String,
    pub offset: usize,
    pub total_entries: usize,
    pub truncated: bool,
}

pub fn adaptive_hash_length(total_lines: usize) -> usize {
    if total_lines > HASH_THRESHOLD {
        LARGE_HASH
    } else {
        SMALL_HASH
    }
}

pub fn line_hash(line: &str, length: usize) -> String {
    hash_text(line, length)
}

pub fn anchor_hash(prev: Option<&str>, line: &str, next: Option<&str>, length: usize) -> String {
    hash_text(
        &format!(
            "{}{}{}{}{}",
            prev.unwrap_or(""),
            SEPARATOR,
            line,
            SEPARATOR,
            next.unwrap_or("")
        ),
        length,
    )
}

pub fn compute_file_rev(raw: &str) -> String {
    hash_text(&raw.replace("\r\n", "\n"), REV_LENGTH)
}

pub fn parse_file(raw: &str) -> ParsedFile {
    let eol = if raw.contains("\r\n") { "\r\n" } else { "\n" };
    let normalized = raw.replace("\r\n", "\n");
    let ends_with_newline = normalized.ends_with('\n');
    let mut lines = if normalized.is_empty() {
        Vec::new()
    } else {
        normalized.split('\n').map(str::to_owned).collect()
    };
    if ends_with_newline {
        lines.pop();
    }
    ParsedFile {
        lines,
        eol,
        ends_with_newline,
    }
}

pub fn stringify_file(parsed: &ParsedFile, lines: &[String]) -> String {
    let mut text = lines.join(parsed.eol);
    if parsed.ends_with_newline {
        text.push_str(parsed.eol);
    }
    text
}

pub fn replacement_lines(content: &str) -> Vec<String> {
    let content = strip_references(content).replace("\r\n", "\n");
    let mut lines = content.split('\n').map(str::to_owned).collect::<Vec<_>>();
    if content.ends_with('\n') {
        lines.pop();
    }
    lines
}

pub fn strip_references(content: &str) -> String {
    content
        .lines()
        .filter_map(|line| {
            if line.starts_with("#HL REV:") {
                return None;
            }
            if let Some(rest) = line.strip_prefix("#HL ") {
                return rest.split_once('|').map(|(_, text)| text.to_string());
            }
            Some(line.to_string())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn parse_ref(reference: &str) -> Result<ParsedRef, String> {
    let text = reference
        .trim()
        .strip_prefix("#HL")
        .unwrap_or(reference.trim())
        .trim()
        .split('|')
        .next()
        .unwrap_or_default();
    let mut parts = text.split('#');
    let line = parts
        .next()
        .ok_or_else(|| "missing line number".to_string())?
        .parse::<usize>()
        .map_err(|_| "invalid line number".to_string())?;
    if line == 0 {
        return Err("line number must be positive".into());
    }
    let hash = parts
        .next()
        .ok_or_else(|| "missing line hash".to_string())?;
    if hash.is_empty() {
        return Err("line hash must not be empty".into());
    }
    let anchor = parts.next().map(str::to_ascii_uppercase);
    if parts.next().is_some() {
        return Err("too many hashline components".into());
    }
    Ok(ParsedRef {
        line,
        hash: hash.to_ascii_uppercase(),
        anchor,
    })
}

pub fn render_file(
    raw: &str,
    path: &std::path::Path,
    offset: usize,
    limit: usize,
    max_line_chars: usize,
    max_bytes: usize,
) -> Result<RenderedFile, String> {
    let parsed = parse_file(raw);
    let total_lines = parsed.lines.len();
    if offset == 0 {
        return Err("offset must be at least 1".into());
    }
    if total_lines > 0 && offset > total_lines {
        return Err(format!("offset {offset} exceeds {total_lines} lines"));
    }
    let hash_length = adaptive_hash_length(total_lines);
    let rev = compute_file_rev(raw);
    let mut body = vec![format!("#HL REV:{rev}")];
    let mut bytes = body[0].len() + 1;
    let start = offset.saturating_sub(1);
    let requested_end = (start + limit).min(total_lines);
    let mut visible_end = start;
    let mut capped = false;
    for index in start..requested_end {
        let line = &parsed.lines[index];
        let display = if line.chars().count() > max_line_chars {
            let shortened = line.chars().take(max_line_chars).collect::<String>();
            format!("{shortened}... (line truncated to {max_line_chars} chars)")
        } else {
            line.clone()
        };
        let entry = format!(
            "#HL {}#{}#{}|{}",
            index + 1,
            line_hash(line, hash_length),
            anchor_hash(
                parsed.lines.get(index.wrapping_sub(1)).map(String::as_str),
                line,
                parsed.lines.get(index + 1).map(String::as_str),
                hash_length
            ),
            display
        );
        if bytes + entry.len() + 1 > max_bytes {
            capped = true;
            break;
        }
        bytes += entry.len() + 1;
        body.push(entry);
        visible_end = index + 1;
    }
    let more = visible_end < total_lines;
    let shown_end = visible_end.max(offset.saturating_sub(1));
    let note = if capped {
        format!(
            "(Output capped at {max_bytes} bytes. Use offset={} to continue.)",
            shown_end + 1
        )
    } else if more {
        format!(
            "(Showing lines {offset}-{shown_end} of {total_lines}. Use offset={} to continue.)",
            shown_end + 1
        )
    } else {
        format!("(End of file - total {total_lines} lines)")
    };
    body.push(note);
    Ok(RenderedFile {
        rev,
        content: format!(
            "<path>{}</path>\n<type>file</type>\n<content>\n{}\n</content>",
            path.display(),
            body.join("\n")
        ),
        line_start: offset,
        line_end: shown_end,
        total_lines,
        truncated: capped || more,
    })
}

pub fn render_directory(
    entries: &[DirectoryEntry],
    path: &std::path::Path,
    offset: usize,
    limit: usize,
) -> RenderedDirectory {
    let width = entries.len().max(10).to_string().len();
    let raw_lines = directory_lines_with_width(entries, width);
    let raw = raw_lines.join("\n");
    let rev = compute_file_rev(&raw);
    let hash_length = adaptive_hash_length(entries.len());
    let start = offset.saturating_sub(1).min(entries.len());
    let end = (start + limit).min(entries.len());
    let lines = entries[start..end]
        .iter()
        .enumerate()
        .map(|(index, _entry)| {
            let real_index = start + index;
            let visible = &raw_lines[real_index];
            format!(
                "#HL {}#{}#{}|{}",
                real_index + 1,
                line_hash(visible, hash_length),
                anchor_hash(
                    raw_lines
                        .get(real_index.wrapping_sub(1))
                        .map(String::as_str),
                    visible,
                    raw_lines.get(real_index + 1).map(String::as_str),
                    hash_length
                ),
                visible
            )
        })
        .collect::<Vec<_>>();
    let truncated = end < entries.len();
    let note = if truncated {
        format!(
            "(Showing {start}-{end} of {} entries. Use offset={} to continue.)",
            entries.len(),
            end + 1
        )
    } else {
        format!("({} entries)", entries.len())
    };
    RenderedDirectory {
        rev: rev.clone(),
        content: format!(
            "<path>{}</path>\n<type>directory</type>\n<entries>\n#HL REV:{rev}\n{}\n{}\n</entries>",
            path.display(),
            lines.join("\n"),
            note
        ),
        offset,
        total_entries: entries.len(),
        truncated,
    }
}

pub fn directory_lines(entries: &[DirectoryEntry]) -> Vec<String> {
    let width = entries.len().max(10).to_string().len();
    directory_lines_with_width(entries, width)
}

fn directory_lines_with_width(entries: &[DirectoryEntry], width: usize) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            format!(
                "{:0width$}|{}",
                entry.id,
                display_entry(entry),
                width = width
            )
        })
        .collect()
}

fn display_entry(entry: &DirectoryEntry) -> String {
    match entry.kind {
        super::types::EntryKind::Directory => format!("{}/", entry.name),
        _ => entry.name.clone(),
    }
}

fn hash_text(text: &str, length: usize) -> String {
    let digest = Sha1::digest(text.as_bytes());
    format!("{:x}", digest)[..length].to_ascii_uppercase()
}
