//! Operations-sidebar model and action surface.
//!
//! This module deliberately contains no filesystem or process access.  The
//! daemon/client is the only authority for repository state and mutations.

use std::collections::HashMap;

use tau_proto::git::{GitBranch, GitFileResult, GitFileStatus};

gpui::actions!(
    operations,
    [
        RefreshOperations,
        StageFile,
        UnstageFile,
        RevertFile,
        KeepFile,
        CreateBranch,
        SwitchBranch
    ]
);

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OperationsModel {
    pub branch: String,
    pub files: Vec<GitFileStatus>,
    pub branches: Vec<GitBranch>,
    pub selected: Option<GitFileResult>,
    keeps: HashMap<String, String>,
    pub acknowledgement: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationsAction {
    Refresh,
    OpenFile(String),
    Stage(String),
    Unstage(String),
    /// Revert is never implicit: callers must dispatch this confirmation.
    RevertConfirmed(String),
    Keep(String),
    CreateBranch(String),
    SwitchBranch(String),
    Acknowledge {
        operation: String,
        acknowledged: bool,
    },
}

impl OperationsModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply_status(&mut self, branch: String, files: Vec<GitFileStatus>) {
        self.branch = branch;
        self.files = files;
        self.selected = None;
    }

    pub fn apply_branches(&mut self, branches: Vec<GitBranch>) {
        self.branches = branches;
    }

    pub fn select_file(&mut self, file: GitFileResult) {
        self.selected = Some(file);
    }

    /// Keep is local to this model and keyed by the current content revision.
    /// A changed revision automatically becomes visible again.
    pub fn is_kept(&self, path: &str, revision: &str) -> bool {
        self.keeps.get(path).is_some_and(|value| value == revision)
    }

    pub fn toggle_keep(&mut self, path: impl Into<String>, revision: impl Into<String>) {
        let path = path.into();
        let revision = revision.into();
        if self.is_kept(&path, &revision) {
            self.keeps.remove(&path);
        } else {
            self.keeps.insert(path, revision);
        }
    }

    pub fn invalidate_revision(&mut self, path: &str, revision: &str) {
        if !self.is_kept(path, revision) {
            self.keeps.remove(path);
        }
    }

    pub fn acknowledge(&mut self, value: bool) {
        self.acknowledgement = Some(value);
    }
}

/// Typed RPC boundary used by the GPUI view.  Implementations belong to the
/// client/backend layer; keeping this trait here makes the model window-free.
pub trait OperationsClient {
    type Error;
    fn status(
        &self,
        project: &str,
    ) -> impl std::future::Future<Output = Result<(String, Vec<GitFileStatus>), Self::Error>>;
    fn file(
        &self,
        project: &str,
        path: &str,
    ) -> impl std::future::Future<Output = Result<GitFileResult, Self::Error>>;
}

/// Typed daemon-backed operations used by the GPUI view. The view supplies the
/// active project and updates local model state; all repository effects stay
/// behind `tau-client`.
pub async fn status(
    client: &tau_client::Client,
    project: impl Into<String>,
    model: &mut OperationsModel,
) -> anyhow::Result<()> {
    let result = client
        .git_status(tau_proto::git::GitStatusParams {
            project: project.into(),
        })
        .await?;
    model.apply_status(result.branch, result.files);
    Ok(())
}

pub async fn file(
    client: &tau_client::Client,
    project: impl Into<String>,
    path: impl Into<String>,
    model: &mut OperationsModel,
) -> anyhow::Result<()> {
    let result = client
        .git_file(tau_proto::git::GitFileParams {
            project: project.into(),
            path: path.into(),
        })
        .await?;
    let revision = content_revision(&result.content);
    model.invalidate_revision(&result.path, &revision);
    model.select_file(result);
    Ok(())
}

pub async fn stage(
    client: &tau_client::Client,
    project: impl Into<String>,
    path: impl Into<String>,
) -> anyhow::Result<()> {
    client
        .git_stage(tau_proto::git::GitPathParams {
            project: project.into(),
            path: path.into(),
        })
        .await
        .map(|_| ())
}

pub async fn unstage(
    client: &tau_client::Client,
    project: impl Into<String>,
    path: impl Into<String>,
) -> anyhow::Result<()> {
    client
        .git_unstage(tau_proto::git::GitPathParams {
            project: project.into(),
            path: path.into(),
        })
        .await
        .map(|_| ())
}

pub async fn revert(
    client: &tau_client::Client,
    project: impl Into<String>,
    path: impl Into<String>,
    confirmed: bool,
) -> anyhow::Result<()> {
    client
        .git_revert(tau_proto::git::GitRevertParams {
            project: project.into(),
            path: path.into(),
            confirmed,
        })
        .await
        .map(|_| ())
}

pub async fn branches(
    client: &tau_client::Client,
    project: impl Into<String>,
    model: &mut OperationsModel,
) -> anyhow::Result<()> {
    let result = client
        .git_branches(tau_proto::git::GitBranchesParams {
            project: project.into(),
        })
        .await?;
    model.branch = result.current;
    model.apply_branches(result.branches);
    Ok(())
}

pub async fn create_branch(
    client: &tau_client::Client,
    project: impl Into<String>,
    name: impl Into<String>,
) -> anyhow::Result<()> {
    client
        .git_branch_create(tau_proto::git::GitBranchCreateParams {
            project: project.into(),
            name: name.into(),
        })
        .await
        .map(|_| ())
}

pub async fn switch_branch(
    client: &tau_client::Client,
    project: impl Into<String>,
    name: impl Into<String>,
) -> anyhow::Result<()> {
    client
        .git_branch_switch(tau_proto::git::GitBranchSwitchParams {
            project: project.into(),
            name: name.into(),
        })
        .await
        .map(|_| ())
}

pub async fn acknowledge(
    client: &tau_client::Client,
    project: impl Into<String>,
    operation: impl Into<String>,
    acknowledged: bool,
    model: &mut OperationsModel,
) -> anyhow::Result<bool> {
    let result = client
        .git_ack(tau_proto::git::GitAckParams {
            project: project.into(),
            operation: operation.into(),
            acknowledged,
        })
        .await?;
    model.acknowledge(result.acknowledged);
    Ok(result.acknowledged)
}

fn content_revision(content: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keep_reappears_after_revision_changes() {
        let mut model = OperationsModel::new();
        model.toggle_keep("a.txt", "one");
        assert!(model.is_kept("a.txt", "one"));
        model.invalidate_revision("a.txt", "two");
        assert!(!model.is_kept("a.txt", "one"));
    }
}
