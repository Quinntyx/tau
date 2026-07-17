use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("{operation} failed for {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{operation} requires approval for path {path}")]
    ApprovalNeeded {
        operation: &'static str,
        path: PathBuf,
        roots: Vec<PathBuf>,
    },
    #[error("invalid pattern {pattern}: {message}")]
    InvalidPattern { pattern: String, message: String },
    #[error("path is not a directory: {0}")]
    NotDirectory(PathBuf),
    #[error("path is not a file: {0}")]
    NotFile(PathBuf),
    #[error("file is binary: {path} ({mime})")]
    BinaryFile { path: PathBuf, mime: String },
    #[error("tool already registered: {0}")]
    DuplicateTool(String),
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("invalid tool input: {0}")]
    InvalidInput(String),
    #[error("tool serialization failed: {0}")]
    Serialization(String),
}

pub(crate) fn io(
    operation: &'static str,
    path: impl Into<PathBuf>,
    source: std::io::Error,
) -> ToolError {
    ToolError::Io {
        operation,
        path: path.into(),
        source,
    }
}
