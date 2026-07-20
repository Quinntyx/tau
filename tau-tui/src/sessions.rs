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
    /// Replace the projection with summaries acknowledged by the daemon.
    /// Records from another project are intentionally discarded here so a
    /// stale response can never leak sessions into the active navigator.
    pub fn load_daemon_sessions(
        &mut self,
        project: ProjectId,
        sessions: impl IntoIterator<Item = (SessionId, String, u64, bool)>,
    ) {
        self.project = Some(project.clone());
        self.sessions = sessions
            .into_iter()
            .map(|(id, title, updated_at, archived)| SessionEntry {
                id,
                project_id: project.clone(),
                title,
                updated_at,
                archived,
            })
            .collect();
        self.selected = 0;
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
        let project = self.project.clone();
        if let Some(s) = self
            .sessions
            .iter_mut()
            .find(|s| &s.id == id && project.as_ref() == Some(&s.project_id))
        {
            s.title = title.into();
        }
    }
    pub fn archive(&mut self, id: &SessionId) {
        let project = self.project.clone();
        if let Some(s) = self
            .sessions
            .iter_mut()
            .find(|s| &s.id == id && project.as_ref() == Some(&s.project_id))
        {
            s.archived = true;
        }
        self.selected = self.selected.min(self.visible().len().saturating_sub(1));
    }
    pub fn restore(&mut self, id: &SessionId) {
        let project = self.project.clone();
        if let Some(s) = self
            .sessions
            .iter_mut()
            .find(|s| &s.id == id && project.as_ref() == Some(&s.project_id))
        {
            s.archived = false;
        }
    }

    pub fn toggle_archived(&mut self) {
        self.show_archived = !self.show_archived;
        self.selected = 0;
    }

    /// Insert a session created by the daemon. IDs are never synthesized in
    /// the presentation layer.
    pub fn new_chat(&mut self, project_id: ProjectId, id: SessionId, now: u64) -> SessionId {
        if self.project.as_ref() != Some(&project_id) {
            self.project = Some(project_id.clone());
            self.selected = 0;
        }
        self.sessions.retain(|session| session.id != id);
        self.sessions.push(SessionEntry {
            id: id.clone(),
            project_id,
            title: String::new(),
            updated_at: now,
            archived: false,
        });
        self.selected = 0;
        id
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
            project: Some(ProjectId::new("p")),
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
    fn restart_selection_and_new_chat_are_project_bound() {
        let mut n = Navigator {
            project: Some(ProjectId::new("p")),
            ..Default::default()
        };
        let id = SessionId::new("daemon-session");
        n.new_chat(ProjectId::new("p"), id.clone(), 10);
        n.restart_selection(Some(id.as_str()));
        assert_eq!(n.selected_id(), Some(id));
        assert_eq!(n.visible()[0].title, "");
    }
}
