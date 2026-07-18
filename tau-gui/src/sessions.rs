//! Client-local model for the flat session navigator.
//!
//! This module intentionally contains no protocol types.  The daemon/client
//! adapter can translate its records into [`SessionRecord`], keeping project
//! identifiers opaque at the GUI boundary.
use std::cmp::Reverse;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProjectId(String);

impl ProjectId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRecord {
    pub id: String,
    pub project: ProjectId,
    pub title: String,
    pub updated_at: i64,
    pub archived: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Navigator {
    records: Vec<SessionRecord>,
    query: String,
    project: Option<ProjectId>,
    show_archived: bool,
    active: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionAction {
    Rename,
    Archive,
    Restore,
}

impl Navigator {
    pub fn new(records: impl IntoIterator<Item = SessionRecord>) -> Self {
        let mut this = Self {
            records: records.into_iter().collect(),
            ..Self::default()
        };
        this.sort();
        this
    }

    pub fn records(&self) -> &[SessionRecord] {
        &self.records
    }
    pub fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }
    pub fn set_query(&mut self, query: impl Into<String>) {
        self.query = query.into();
    }
    pub fn set_project(&mut self, project: Option<ProjectId>) {
        self.project = project;
    }
    pub fn set_show_archived(&mut self, show: bool) {
        self.show_archived = show;
    }

    /// Returns one flat, newest-updated-first sequence.  No grouping or
    /// touched-file expansion is performed here (or required by the view).
    pub fn visible(&self) -> Vec<&SessionRecord> {
        let query = self.query.trim().to_lowercase();
        self.records
            .iter()
            .filter(|record| {
                (self.show_archived || !record.archived)
                    && self.project.as_ref().is_none_or(|p| p == &record.project)
                    && (query.is_empty()
                        || record.title.to_lowercase().contains(&query)
                        || record.id.to_lowercase().contains(&query))
            })
            .collect()
    }

    pub fn select(&mut self, id: impl Into<String>) {
        self.active = Some(id.into());
    }
    pub fn restart_selection(&mut self, persisted_id: Option<&str>) {
        self.active = persisted_id.and_then(|id| {
            self.records
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.id.clone())
        });
    }

    pub fn apply(&mut self, id: &str, action: SessionAction, title: Option<String>) -> bool {
        let Some(record) = self.records.iter_mut().find(|r| r.id == id) else {
            return false;
        };
        match action {
            SessionAction::Rename => {
                if let Some(title) = title {
                    record.title = title;
                } else {
                    return false;
                }
            }
            SessionAction::Archive => record.archived = true,
            SessionAction::Restore => record.archived = false,
        }
        self.sort();
        true
    }

    /// New Chat creates an immediately selectable empty record bound to the
    /// current project; the first turn may subsequently fill in its title.
    pub fn new_chat(&mut self, project: ProjectId, now: i64) -> String {
        let id = format!("local-{}", now);
        self.records.push(SessionRecord {
            id: id.clone(),
            project,
            title: String::new(),
            updated_at: now,
            archived: false,
        });
        self.active = Some(id.clone());
        self.sort();
        id
    }

    fn sort(&mut self) {
        self.records
            .sort_by_key(|r| (Reverse(r.updated_at), r.id.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(
        id: &str,
        project: &str,
        title: &str,
        updated_at: i64,
        archived: bool,
    ) -> SessionRecord {
        SessionRecord {
            id: id.into(),
            project: ProjectId::new(project),
            title: title.into(),
            updated_at,
            archived,
        }
    }

    #[test]
    fn filters_flat_and_newest_first() {
        let mut n = Navigator::new([
            record("a", "p", "Alpha", 1, false),
            record("b", "p", "Beta", 3, false),
            record("c", "q", "Gamma", 4, false),
        ]);
        n.set_project(Some(ProjectId::new("p")));
        n.set_query("alp");
        assert_eq!(
            n.visible()
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a"]
        );
        n.set_query("");
        assert_eq!(
            n.visible()
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "a"]
        );
    }

    #[test]
    fn actions_and_restart_are_local_and_archived_hidden() {
        let mut n = Navigator::new([
            record("a", "p", "Alpha", 1, false),
            record("z", "p", "Old", 0, true),
        ]);
        assert!(n.apply("a", SessionAction::Rename, Some("Renamed".into())));
        assert!(n.apply("a", SessionAction::Archive, None));
        assert!(n.visible().is_empty());
        n.set_show_archived(true);
        assert_eq!(n.visible().len(), 2);
        n.restart_selection(Some("a"));
        assert_eq!(n.active(), Some("a"));
        n.apply("a", SessionAction::Restore, None);
        assert!(!n.records()[0].archived);
    }

    #[test]
    fn new_chat_is_empty_and_active() {
        let mut n = Navigator::default();
        let id = n.new_chat(ProjectId::new("p"), 42);
        assert_eq!(n.active(), Some(id.as_str()));
        assert_eq!(n.records()[0].title, "");
    }
}
