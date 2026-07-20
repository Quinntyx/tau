//! GPUI-facing session navigator model and daemon action boundary.
//!
//! The model deliberately owns no project registry knowledge.  The daemon
//! adapter translates its records into [`SessionSummary`] and executes the
//! typed intents returned by this module.
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
pub struct SessionSummary {
    pub id: String,
    pub project_id: ProjectId,
    pub title: Option<String>,
    pub updated_at: i64,
    pub archived: bool,
}

/// Compatibility name for adapters that used the original model name.
pub type SessionRecord = SessionSummary;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionIntent {
    Create {
        project_id: ProjectId,
    },
    List {
        project_id: ProjectId,
        include_archived: bool,
    },
    Select {
        project_id: ProjectId,
        session_id: String,
    },
    Rename {
        project_id: ProjectId,
        session_id: String,
        title: String,
    },
    Archive {
        project_id: ProjectId,
        session_id: String,
    },
    Restore {
        project_id: ProjectId,
        session_id: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Navigator {
    records: Vec<SessionSummary>,
    query: String,
    project: Option<ProjectId>,
    show_archived: bool,
    selected: Option<String>,
}

impl Navigator {
    pub fn new(records: impl IntoIterator<Item = SessionSummary>) -> Self {
        let mut this = Self {
            records: records.into_iter().collect(),
            ..Self::default()
        };
        this.sort();
        this
    }
    pub fn records(&self) -> &[SessionSummary] {
        &self.records
    }
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }
    pub fn active(&self) -> Option<&str> {
        self.selected()
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
    pub fn show_archived(&self) -> bool {
        self.show_archived
    }
    pub fn visible(&self) -> Vec<&SessionSummary> {
        let query = self.query.trim().to_lowercase();
        self.records
            .iter()
            .filter(|r| {
                (self.show_archived || !r.archived)
                    && self.project.as_ref().is_none_or(|p| p == &r.project_id)
                    && (query.is_empty()
                        || r.title
                            .as_deref()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&query)
                        || r.id.to_lowercase().contains(&query))
            })
            .collect()
    }
    pub fn ingest(&mut self, records: impl IntoIterator<Item = SessionSummary>) {
        self.records = records.into_iter().collect();
        self.sort();
        if self
            .selected
            .as_ref()
            .is_some_and(|id| !self.records.iter().any(|r| &r.id == id))
        {
            self.selected = None;
        }
    }
    pub fn restore_selection(&mut self, id: Option<&str>) {
        self.selected = id.and_then(|id| {
            self.records
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.id.clone())
        });
    }
    pub fn restart_selection(&mut self, id: Option<&str>) {
        self.restore_selection(id);
    }
    pub fn intent(&self, intent: SessionIntent) -> SessionIntent {
        intent
    }
    pub fn create_intent(&self, project_id: ProjectId) -> SessionIntent {
        SessionIntent::Create { project_id }
    }
    pub fn list_intent(&self, project_id: ProjectId) -> SessionIntent {
        SessionIntent::List {
            project_id,
            include_archived: self.show_archived,
        }
    }
    pub fn select_intent(&mut self, record: &SessionSummary) -> SessionIntent {
        self.selected = Some(record.id.clone());
        SessionIntent::Select {
            project_id: record.project_id.clone(),
            session_id: record.id.clone(),
        }
    }
    pub fn rename_intent(
        &self,
        record: &SessionSummary,
        title: impl Into<String>,
    ) -> SessionIntent {
        SessionIntent::Rename {
            project_id: record.project_id.clone(),
            session_id: record.id.clone(),
            title: title.into(),
        }
    }
    pub fn archive_intent(&self, record: &SessionSummary) -> SessionIntent {
        SessionIntent::Archive {
            project_id: record.project_id.clone(),
            session_id: record.id.clone(),
        }
    }
    pub fn restore_intent(&self, record: &SessionSummary) -> SessionIntent {
        SessionIntent::Restore {
            project_id: record.project_id.clone(),
            session_id: record.id.clone(),
        }
    }
    fn sort(&mut self) {
        self.records
            .sort_by_key(|r| (Reverse(r.updated_at), r.id.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(
        id: &str,
        project: &str,
        title: &str,
        updated_at: i64,
        archived: bool,
    ) -> SessionSummary {
        SessionSummary {
            id: id.into(),
            project_id: ProjectId::new(project),
            title: Some(title.into()),
            updated_at,
            archived,
        }
    }

    #[test]
    fn visible_rows_are_project_filtered_searchable_and_newest_first() {
        let mut navigator = Navigator::new([
            summary("old", "p", "Alpha", 1, false),
            summary("new", "p", "Beta", 3, false),
            summary("other", "q", "Beta", 4, false),
        ]);
        navigator.set_project(Some(ProjectId::new("p")));
        assert_eq!(
            navigator
                .visible()
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            ["new", "old"]
        );
        navigator.set_query("alp");
        assert_eq!(navigator.visible()[0].id, "old");
    }

    #[test]
    fn intents_carry_opaque_project_and_session_ids() {
        let record = summary("s", "opaque", "Title", 1, false);
        let mut navigator = Navigator::new([record.clone()]);
        assert_eq!(
            navigator.select_intent(&record),
            SessionIntent::Select {
                project_id: ProjectId::new("opaque"),
                session_id: "s".into()
            }
        );
        assert_eq!(navigator.selected(), Some("s"));
        assert_eq!(
            navigator.rename_intent(&record, "New"),
            SessionIntent::Rename {
                project_id: ProjectId::new("opaque"),
                session_id: "s".into(),
                title: "New".into()
            }
        );
        assert_eq!(
            navigator.archive_intent(&record),
            SessionIntent::Archive {
                project_id: ProjectId::new("opaque"),
                session_id: "s".into()
            }
        );
    }

    #[test]
    fn persisted_selection_restores_only_existing_sessions() {
        let mut navigator = Navigator::new([summary("s", "p", "Title", 1, false)]);
        navigator.restore_selection(Some("s"));
        assert_eq!(navigator.selected(), Some("s"));
        navigator.restore_selection(Some("missing"));
        assert_eq!(navigator.selected(), None);
    }
}
