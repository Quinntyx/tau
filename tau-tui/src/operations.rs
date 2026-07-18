//! Typed, daemon-backed operations sidebar.  This module intentionally has no
//! filesystem or process access: all workspace effects cross the tau protocol.
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use std::collections::HashMap;
use tau_client::Client;
use tau_proto::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsState {
    pub project: String,
    pub branch: String,
    pub files: Vec<GitFileStatus>,
    pub selected: usize,
    pub content: Option<GitFileResult>,
    pub branches: Vec<GitBranch>,
    pub acknowledged: HashMap<String, bool>,
    keeps: HashMap<String, u64>,
}
impl OperationsState {
    pub fn new(project: impl Into<String>) -> Self {
        Self {
            project: project.into(),
            branch: String::new(),
            files: vec![],
            selected: 0,
            content: None,
            branches: vec![],
            acknowledged: HashMap::new(),
            keeps: HashMap::new(),
        }
    }
    pub fn path(&self) -> Option<&str> {
        self.files.get(self.selected).map(|f| f.path.as_str())
    }
    pub fn is_kept(&self, path: &str, revision: u64) -> bool {
        self.keeps.get(path) == Some(&revision)
    }
    pub fn revision(content: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut h);
        h.finish()
    }
}
impl Default for OperationsState {
    fn default() -> Self {
        Self::new(".")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Refresh,
    Select(usize),
    Open(String),
    Stage(String),
    Unstage(String),
    Revert(String),
    Keep(String),
    CreateBranch(String),
    SwitchBranch(String),
    Acknowledge(String, bool),
}

pub fn reduce(state: &mut OperationsState, action: Action) {
    match action {
        Action::Select(i) => state.selected = i.min(state.files.len().saturating_sub(1)),
        Action::Keep(path) => {
            if let Some(file) = state.content.as_ref().filter(|f| f.path == path) {
                state
                    .keeps
                    .insert(path, OperationsState::revision(&file.content));
            }
        }
        Action::Open(path) => state.content = state.content.take().filter(|f| f.path == path),
        Action::Acknowledge(op, yes) => {
            state.acknowledged.insert(op, yes);
        }
        _ => {}
    }
}

pub async fn status(client: &Client, state: &mut OperationsState) -> anyhow::Result<()> {
    let r = client
        .git_status(GitStatusParams {
            project: state.project.clone(),
        })
        .await?;
    state.branch = r.branch;
    state.files = r.files;
    state.selected = state.selected.min(state.files.len().saturating_sub(1));
    Ok(())
}
pub async fn file(
    client: &Client,
    state: &mut OperationsState,
    path: impl Into<String>,
) -> anyhow::Result<()> {
    let path = path.into();
    let r = client
        .git_file(GitFileParams {
            project: state.project.clone(),
            path,
        })
        .await?;
    let revision = OperationsState::revision(&r.content);
    if state
        .content
        .as_ref()
        .map(|old| OperationsState::revision(&old.content))
        != Some(revision)
    {
        state.keeps.remove(&r.path);
    }
    state.content = Some(r);
    Ok(())
}
pub async fn stage(client: &Client, state: &OperationsState, path: String) -> anyhow::Result<()> {
    client
        .git_stage(GitPathParams {
            project: state.project.clone(),
            path,
        })
        .await
        .map(|_| ())
}
pub async fn unstage(client: &Client, state: &OperationsState, path: String) -> anyhow::Result<()> {
    client
        .git_unstage(GitPathParams {
            project: state.project.clone(),
            path,
        })
        .await
        .map(|_| ())
}
pub async fn revert(client: &Client, state: &OperationsState, path: String) -> anyhow::Result<()> {
    let p = GitRevertParams {
        project: state.project.clone(),
        path,
        confirmed: true,
    };
    anyhow::ensure!(
        p.validate(),
        "revert requires confirmation and a project-relative path"
    );
    client.git_revert(p).await.map(|_| ())
}
pub async fn branches(client: &Client, state: &mut OperationsState) -> anyhow::Result<()> {
    let r = client
        .git_branches(GitBranchesParams {
            project: state.project.clone(),
        })
        .await?;
    state.branch = r.current;
    state.branches = r.branches;
    Ok(())
}
pub async fn create_branch(
    client: &Client,
    state: &OperationsState,
    name: String,
) -> anyhow::Result<()> {
    client
        .git_branch_create(GitBranchCreateParams {
            project: state.project.clone(),
            name,
        })
        .await
        .map(|_| ())
}
pub async fn switch_branch(
    client: &Client,
    state: &OperationsState,
    name: String,
) -> anyhow::Result<()> {
    client
        .git_branch_switch(GitBranchSwitchParams {
            project: state.project.clone(),
            name,
        })
        .await
        .map(|_| ())
}
pub async fn acknowledge(
    client: &Client,
    state: &OperationsState,
    operation: String,
    acknowledged: bool,
) -> anyhow::Result<bool> {
    Ok(client
        .git_ack(GitAckParams {
            project: state.project.clone(),
            operation,
            acknowledged,
        })
        .await?
        .acknowledged)
}

pub fn render(frame: &mut Frame, area: Rect, state: &OperationsState) {
    let items = state
        .files
        .iter()
        .map(|f| ListItem::new(format!("{} {}", if f.staged { "●" } else { "○" }, f.path)))
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title(format!("Operations [{}]", state.branch))
                .borders(Borders::ALL),
        ),
        area,
    );
    if let Some(c) = &state.content {
        let r = Rect {
            x: area.x + area.width / 2,
            y: area.y,
            width: area.width - area.width / 2,
            height: area.height,
        };
        frame.render_widget(
            Paragraph::new(c.diff.as_str()).block(
                Block::default()
                    .title(c.path.as_str())
                    .borders(Borders::ALL),
            ),
            r,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_is_invalidated_when_file_revision_changes() {
        let mut state = OperationsState::new("/project");
        state.content = Some(GitFileResult {
            path: "tracked.txt".into(),
            content: "one".into(),
            diff: String::new(),
        });
        reduce(&mut state, Action::Keep("tracked.txt".into()));
        let first = OperationsState::revision("one");
        assert!(state.is_kept("tracked.txt", first));
        state.content = Some(GitFileResult {
            path: "tracked.txt".into(),
            content: "two".into(),
            diff: String::new(),
        });
        assert!(!state.is_kept("tracked.txt", OperationsState::revision("two")));
    }

    #[test]
    fn operations_render_snapshot() {
        let mut state = OperationsState::new("/project");
        state.branch = "main".into();
        state.files = vec![GitFileStatus {
            path: "tracked.txt".into(),
            staged: true,
            modified: true,
            untracked: false,
        }];
        state.content = Some(GitFileResult {
            path: "tracked.txt".into(),
            content: "changed\n".into(),
            diff: "@@ -1 +1 @@\n-base\n+changed\n".into(),
        });
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(72, 8)).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &state))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Operations [main]"));
        assert!(text.contains("tracked.txt"));
        assert!(text.contains("changed"));
    }
}
