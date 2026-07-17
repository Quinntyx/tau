use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::mutation::MutationCoordinator;
use super::policy::AccessPolicy;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolLimits {
    pub read_lines: usize,
    pub read_bytes: usize,
    pub max_line_chars: usize,
    pub search_matches: usize,
    pub glob_results: usize,
    pub directory_entries: usize,
    pub binary_bytes: usize,
    pub bash_lines: usize,
    pub bash_bytes: usize,
    pub bash_timeout_seconds: u64,
}

impl Default for ToolLimits {
    fn default() -> Self {
        Self {
            read_lines: 2_000,
            read_bytes: 50 * 1024,
            max_line_chars: 2_000,
            search_matches: 100,
            glob_results: 100,
            directory_entries: 2_000,
            binary_bytes: 50 * 1024,
            bash_lines: 2_000,
            bash_bytes: 50 * 1024,
            bash_timeout_seconds: 120,
        }
    }
}

#[derive(Clone)]
pub struct ToolContext {
    pub cwd: PathBuf,
    pub policy: AccessPolicy,
    pub limits: ToolLimits,
    pub mutation: MutationCoordinator,
}

impl ToolContext {
    pub fn new(cwd: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let cwd = cwd.into();
        Ok(Self {
            policy: AccessPolicy::for_cwd(&cwd)?,
            cwd,
            limits: ToolLimits::default(),
            mutation: MutationCoordinator::default(),
        })
    }

    pub fn with_limits(mut self, limits: ToolLimits) -> Self {
        self.limits = limits;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub id: usize,
    pub name: String,
    pub kind: EntryKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRead {
    pub path: PathBuf,
    pub rev: String,
    pub content: String,
    pub line_start: usize,
    pub line_end: usize,
    pub total_lines: usize,
    pub truncated: bool,
    #[serde(skip)]
    pub rendered: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryRead {
    pub path: PathBuf,
    pub rev: String,
    pub entries: Vec<DirectoryEntry>,
    pub offset: usize,
    pub total_entries: usize,
    pub truncated: bool,
    #[serde(skip)]
    pub rendered: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryRead {
    pub path: PathBuf,
    pub mime: String,
    pub bytes: Vec<u8>,
    pub truncated: bool,
    #[serde(skip)]
    pub rendered: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ReadOutput {
    File(FileRead),
    Directory(DirectoryRead),
    Binary(BinaryRead),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListOutput {
    pub path: PathBuf,
    pub entries: Vec<DirectoryEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobOutput {
    pub root: PathBuf,
    pub entries: Vec<PathBuf>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepMatch {
    pub path: PathBuf,
    pub line: usize,
    pub offset: usize,
    pub text: String,
    pub submatches: Vec<Submatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Submatch {
    pub text: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepOutput {
    pub matches: Vec<GrepMatch>,
    pub truncated: bool,
}
