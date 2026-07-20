//! Client-local model for the flat session navigator.
use std::cmp::Reverse;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectId(String);
impl ProjectId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(String);
impl SessionId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    pub id: SessionId,
    pub project_id: ProjectId,
    pub title: String,
    pub updated_at: u64,
    pub archived: bool,
}

/// Convert the daemon's typed session summary into the navigator projection.
/// The navigator never allocates or derives durable identifiers locally.
pub fn entry_from_summary(summary: &tau_client::SessionSummary) -> SessionEntry {
    SessionEntry {
        id: SessionId::new(summary.session_id.as_str()),
        project_id: ProjectId::new(summary.project_id.as_str()),
        title: summary
            .title
            .clone()
            .unwrap_or_else(|| "new conversation".into()),
        updated_at: summary.updated_at.max(0) as u64,
        archived: summary.archived_at.is_some(),
    }
}

#[derive(Debug, Clone, Default)]
pub struct Navigator {
    pub project: Option<ProjectId>,
    pub query: String,
    pub sessions: Vec<SessionEntry>,
    pub selected: usize,
    pub open: bool,
    pub show_archived: bool,
}

impl Navigator {
    /// Replace the daemon snapshot, retaining selection only when its
    /// durable daemon id is still present in the active project projection.
    ///
    /// The project is supplied by the request/response envelope rather than
    /// inferred from the first row; an empty response therefore cannot leak a
    /// previous project's selection.
    pub fn replace_sessions(
        &mut self,
        project: ProjectId,
        sessions: Vec<SessionEntry>,
        persisted_id: Option<&str>,
    ) {
        self.project = Some(project.clone());
        self.sessions = sessions
            .into_iter()
            .filter(|session| session.project_id == project)
            .collect();
        self.restart_selection(persisted_id);
    }

    pub fn visible(&self) -> Vec<&SessionEntry> {
        let q = self.query.to_lowercase();
        let mut result: Vec<_> = self
            .sessions
            .iter()
            .filter(|s| {
                self.project.as_ref().is_none_or(|p| p == &s.project_id)
                    && (self.show_archived || !s.archived)
                    && (q.is_empty()
                        || s.title.to_lowercase().contains(&q)
                        || s.id.as_str().to_lowercase().contains(&q))
            })
            .collect();
        result.sort_by_key(|s| Reverse(s.updated_at));
        result
    }
    pub fn select_delta(&mut self, delta: i8) {
        let count = self.visible().len();
        if count > 0 {
            self.selected = if delta < 0 {
                self.selected.saturating_sub(1)
            } else {
                (self.selected + 1).min(count - 1)
            };
        }
    }
    pub fn selected_id(&self) -> Option<SessionId> {
        self.visible().get(self.selected).map(|s| s.id.clone())
    }
    pub fn restart_selection(&mut self, persisted_id: Option<&str>) {
        self.selected = persisted_id
            .and_then(|id| {
                self.visible()
                    .iter()
                    .position(|entry| entry.id.as_str() == id)
            })
            .unwrap_or(0);
    }
    pub fn rename(&mut self, id: &SessionId, title: impl Into<String>) {
        if let Some(s) = self.sessions.iter_mut().find(|s| &s.id == id) {
            s.title = title.into();
        }
    }
    pub fn archive(&mut self, id: &SessionId) {
        if let Some(s) = self.sessions.iter_mut().find(|s| &s.id == id) {
            s.archived = true;
        }
        self.selected = self.selected.min(self.visible().len().saturating_sub(1));
    }
    pub fn restore(&mut self, id: &SessionId) {
        if let Some(s) = self.sessions.iter_mut().find(|s| &s.id == id) {
            s.archived = false;
        }
        self.selected = self.selected.min(self.visible().len().saturating_sub(1));
    }

    pub fn toggle_archived(&mut self) {
        self.show_archived = !self.show_archived;
        self.selected = 0;
    }

    /// Prepare the navigator for a daemon-backed new-chat request.
    ///
    /// Creation is asynchronous, so no placeholder entry (and, in
    /// particular, no fabricated local identifier) is inserted here.  The
    /// acknowledged `SessionCreated` response supplies the durable entry.
    pub fn prepare_new_chat(&mut self, project_id: ProjectId) {
        self.project = Some(project_id);
        self.selected = 0;
        self.open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn item(id: &str, title: &str, updated_at: u64) -> SessionEntry {
        SessionEntry {
            id: SessionId::new(id),
            project_id: ProjectId::new("p"),
            title: title.into(),
            updated_at,
            archived: false,
        }
    }
    #[test]
    fn filters_project_search_and_sorts_newest() {
        let mut n = Navigator {
            project: Some(ProjectId::new("p")),
            sessions: vec![item("old", "Old", 1), item("new", "Find me", 2)],
            ..Default::default()
        };
        assert_eq!(n.visible()[0].id.as_str(), "new");
        n.query = "find".into();
        assert_eq!(n.visible().len(), 1);
    }
    #[test]
    fn archived_is_hidden_but_restore_recovers() {
        let mut n = Navigator {
            sessions: vec![item("x", "X", 1)],
            ..Default::default()
        };
        n.archive(&SessionId::new("x"));
        assert!(n.visible().is_empty());
        n.toggle_archived();
        assert_eq!(n.visible().len(), 1);
        n.restore(&SessionId::new("x"));
        assert_eq!(n.visible().len(), 1);
    }

    #[test]
    fn new_chat_does_not_fabricate_an_id() {
        let mut n = Navigator {
            project: Some(ProjectId::new("p")),
            ..Default::default()
        };
        n.prepare_new_chat(ProjectId::new("p"));
        assert!(n.sessions.is_empty());
        assert_eq!(n.selected_id(), None);
    }

    #[test]
    fn replacement_is_project_bound_and_restarts_by_daemon_id() {
        let mut n = Navigator::default();
        n.replace_sessions(
            ProjectId::new("p"),
            vec![
                SessionEntry {
                    project_id: ProjectId::new("other"),
                    ..item("wrong", "Wrong", 3)
                },
                item("kept", "Kept", 2),
            ],
            Some("kept"),
        );
        assert_eq!(n.sessions.len(), 1);
        assert_eq!(n.selected_id().unwrap().as_str(), "kept");
    }
}
