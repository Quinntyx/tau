//! Typed Git operations exposed by the daemon.
//!
//! Paths in this module are always relative to the selected project.  The
//! daemon is responsible for resolving them and must not allow them to escape
//! that project.
use serde::{Deserialize, Serialize};

pub const METHOD_GIT_STATUS: &str = "git.status";
pub const METHOD_GIT_FILE: &str = "git.file";
pub const METHOD_GIT_STAGE: &str = "git.stage";
pub const METHOD_GIT_UNSTAGE: &str = "git.unstage";
pub const METHOD_GIT_REVERT: &str = "git.revert";
pub const METHOD_GIT_ACK: &str = "git.ack";
pub const METHOD_GIT_BRANCHES: &str = "git.branches";
pub const METHOD_GIT_BRANCH_CREATE: &str = "git.branch.create";
pub const METHOD_GIT_BRANCH_SWITCH: &str = "git.branch.switch";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStatusParams {
    pub project: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStatusResult {
    pub branch: String,
    pub files: Vec<GitFileStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitFileParams {
    pub project: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitFileResult {
    pub path: String,
    pub content: String,
    pub diff: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitPathParams {
    pub project: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRevertParams {
    pub project: String,
    pub path: String,
    pub confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBranchesParams {
    pub project: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBranchesResult {
    pub current: String,
    pub branches: Vec<GitBranch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBranchCreateParams {
    pub project: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBranchSwitchParams {
    pub project: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitAckParams {
    pub project: String,
    pub operation: String,
    pub acknowledged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitAckResult {
    pub acknowledged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitFileStatus {
    pub path: String,
    pub staged: bool,
    pub modified: bool,
    pub untracked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBranch {
    pub name: String,
    pub current: bool,
}

impl GitPathParams {
    pub fn validate_path(&self) -> bool {
        is_project_path(&self.path)
    }
}
impl GitRevertParams {
    pub fn validate(&self) -> bool {
        self.confirmed && is_project_path(&self.path)
    }
}
fn is_project_path(path: &str) -> bool {
    !path.trim().is_empty()
        && path != "."
        && !path.starts_with('/')
        && !path.split('/').any(|p| p == "..")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_round_trips_json() {
        let value = GitFileResult {
            path: "src/lib.rs".into(),
            content: "x".into(),
            diff: "+x".into(),
        };
        assert_eq!(
            serde_json::from_value::<GitFileResult>(serde_json::to_value(value.clone()).unwrap())
                .unwrap(),
            value
        );
    }
    #[test]
    fn revert_requires_explicit_confirmation_and_safe_path() {
        let base = GitRevertParams {
            project: "demo".into(),
            path: "src/lib.rs".into(),
            confirmed: true,
        };
        assert!(base.validate());
        assert!(
            !GitRevertParams {
                confirmed: false,
                ..base.clone()
            }
            .validate()
        );
        assert!(
            !GitRevertParams {
                path: "../secret".into(),
                ..base
            }
            .validate()
        );
    }
}
