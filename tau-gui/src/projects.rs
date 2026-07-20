//! Client-local project registry.
//!
//! This module deliberately contains no transport or server API.  A project is
//! a GUI concern: the registry remembers workspace groups and which one the
//! client last displayed.

use std::collections::BTreeMap;
use std::path::PathBuf;

pub type ProjectId = String;

/// The lifecycle state persisted alongside a project record.
///
/// `Project` keeps its historical `inactive` field for compatibility with the
/// shell projection, while this type gives callers a typed way to inspect the
/// state without manufacturing a "Current project" record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectAvailability {
    Active,
    Inactive,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub root: PathBuf,
    pub inactive: bool,
}

impl Project {
    /// Convert the daemon's authoritative record into the GUI domain shape.
    /// The GUI never invents IDs when hydrating this boundary.
    pub fn from_daemon(record: &tau_proto::projects::Project) -> Self {
        Self {
            id: record.id.clone(),
            name: record.name.clone(),
            root: PathBuf::from(&record.root),
            inactive: !record.active,
        }
    }

    pub fn availability(&self) -> ProjectAvailability {
        if self.inactive {
            ProjectAvailability::Inactive
        } else {
            ProjectAvailability::Active
        }
    }

    pub fn is_active(&self) -> bool {
        self.availability() == ProjectAvailability::Active
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectState {
    pub projects: BTreeMap<ProjectId, Project>,
    pub active_project_id: Option<ProjectId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InactiveRegistration {
    ReactivateExisting,
    CreateNew,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectAction {
    Create {
        name: String,
        root: PathBuf,
    },
    Select(ProjectId),
    Update {
        id: ProjectId,
        name: String,
        root: PathBuf,
    },
    Unregister(ProjectId),
    Reactivate(ProjectId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectError {
    NotFound(ProjectId),
    AlreadyRegistered(PathBuf),
    DaemonRequired,
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "project '{id}' was not found or is inactive"),
            Self::AlreadyRegistered(root) => {
                write!(f, "project root is already registered: {}", root.display())
            }
            Self::DaemonRequired => write!(f, "project changes require the daemon registry"),
        }
    }
}

impl std::error::Error for ProjectError {}

impl ProjectState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a record returned by the daemon registry.  Project IDs are never
    /// derived from paths in this projection.
    pub fn insert_daemon_project(
        &mut self,
        id: ProjectId,
        name: String,
        root: PathBuf,
        inactive: bool,
    ) {
        if id.trim().is_empty() {
            return;
        }
        self.projects.insert(
            id.clone(),
            Project {
                id: id.clone(),
                name,
                root,
                inactive,
            },
        );
        if !inactive && self.active_project_id.is_none() {
            self.active_project_id = Some(id);
        }
    }

    /// Hydrate solely from records returned by the daemon and restore only an
    /// active preferred selection. This is the integration seam used by GUI
    /// adapters after `project.list` completes.
    pub fn from_daemon(
        records: impl IntoIterator<Item = tau_proto::projects::Project>,
        preferred: Option<&str>,
    ) -> Self {
        let mut state = Self::new();
        for record in records {
            let project = Project::from_daemon(&record);
            state.projects.insert(project.id.clone(), project);
        }
        state.restore_active(preferred);
        state
    }

    /// Replace the projection after a daemon mutation or refresh.
    pub fn replace_from_daemon(
        &mut self,
        records: impl IntoIterator<Item = tau_proto::projects::Project>,
        preferred: Option<&str>,
    ) {
        *self = Self::from_daemon(records, preferred);
    }

    /// Restore the last selection only when it still names an active project.
    pub fn restore_active(&mut self, saved: Option<&str>) -> Option<&Project> {
        self.active_project_id = saved.and_then(|id| {
            self.projects
                .get(id)
                .filter(|project| !project.inactive)
                .map(|_| id.to_owned())
        });
        self.active()
    }

    pub fn active(&self) -> Option<&Project> {
        self.active_project_id
            .as_ref()
            .and_then(|id| self.projects.get(id))
    }

    /// The only project identifier that should be written as the last-active
    /// preference. In particular, inactive records never leak into storage.
    pub fn persisted_active_id(&self) -> Option<&str> {
        self.active().map(|project| project.id.as_str())
    }

    /// Return records suitable for a persistence/UI projection without adding
    /// a synthetic placeholder when the registry is empty.
    pub fn records(&self) -> impl Iterator<Item = &Project> {
        self.projects.values()
    }

    pub fn apply(&mut self, action: ProjectAction) -> Result<Option<ProjectId>, ProjectError> {
        match action {
            ProjectAction::Create { name, root } => {
                let id = self.create(name, root)?;
                Ok(Some(id))
            }
            ProjectAction::Select(id) => {
                self.select(&id)?;
                Ok(Some(id))
            }
            ProjectAction::Update { id, name, root } => {
                self.update(&id, name, root)?;
                Ok(Some(id))
            }
            ProjectAction::Unregister(id) => {
                self.unregister(&id)?;
                Ok(None)
            }
            ProjectAction::Reactivate(id) => {
                self.reactivate(&id)?;
                Ok(Some(id))
            }
        }
    }

    pub fn create(&mut self, name: String, root: PathBuf) -> Result<ProjectId, ProjectError> {
        let _ = (name, root);
        Err(ProjectError::DaemonRequired)
    }

    pub fn register(
        &mut self,
        name: String,
        root: PathBuf,
        choice: InactiveRegistration,
    ) -> Result<ProjectId, ProjectError> {
        if let Some(existing) = self
            .projects
            .values()
            .find(|p| p.root == root && p.availability() == ProjectAvailability::Inactive)
            .map(|p| p.id.clone())
        {
            return match choice {
                InactiveRegistration::ReactivateExisting => {
                    self.reactivate(&existing)?;
                    Ok(existing)
                }
                InactiveRegistration::CreateNew => self.create(name, root),
            };
        }
        self.create(name, root)
    }

    pub fn select(&mut self, id: &str) -> Result<(), ProjectError> {
        let project = self
            .projects
            .get(id)
            .ok_or_else(|| ProjectError::NotFound(id.into()))?;
        if project.inactive {
            return Err(ProjectError::NotFound(id.into()));
        }
        self.active_project_id = Some(id.into());
        Ok(())
    }

    pub fn update(&mut self, id: &str, name: String, root: PathBuf) -> Result<(), ProjectError> {
        let project = self
            .projects
            .get_mut(id)
            .ok_or_else(|| ProjectError::NotFound(id.into()))?;
        project.name = name;
        project.root = root;
        Ok(())
    }

    pub fn unregister(&mut self, id: &str) -> Result<(), ProjectError> {
        let project = self
            .projects
            .get_mut(id)
            .ok_or_else(|| ProjectError::NotFound(id.into()))?;
        project.inactive = true;
        if self.active_project_id.as_deref() == Some(id) {
            self.active_project_id = None;
        }
        Ok(())
    }

    pub fn reactivate(&mut self, id: &str) -> Result<(), ProjectError> {
        let project = self
            .projects
            .get_mut(id)
            .ok_or_else(|| ProjectError::NotFound(id.into()))?;
        project.inactive = false;
        self.active_project_id = Some(id.into());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scripted_lifecycle_and_restore() {
        let mut state = ProjectState::new();
        let id = "daemon-one".to_owned();
        state.insert_daemon_project(id.clone(), "one".into(), PathBuf::from("/one"), false);
        state
            .update(&id, "renamed".into(), PathBuf::from("/uno"))
            .unwrap();
        state.unregister(&id).unwrap();
        assert!(state.restore_active(Some(&id)).is_none());
        let restored = state.register(
            "one".into(),
            PathBuf::from("/uno"),
            InactiveRegistration::ReactivateExisting,
        );
        assert_eq!(restored.unwrap(), id);
        assert_eq!(state.active().unwrap().name, "renamed");
    }

    #[test]
    fn inactive_registration_choice_can_create_a_second_id_for_same_root() {
        let mut state = ProjectState::default();
        let old = "daemon-old".to_owned();
        state.insert_daemon_project(old.clone(), "old".into(), PathBuf::from("/shared"), false);
        state.unregister(&old).unwrap();
        assert_eq!(
            state.register(
                "new".into(),
                PathBuf::from("/shared"),
                InactiveRegistration::CreateNew
            ),
            Err(ProjectError::DaemonRequired)
        );
    }

    #[test]
    fn daemon_hydration_preserves_ids_and_preferred_active_only() {
        let records = vec![
            tau_proto::projects::Project {
                id: "p_active".into(),
                name: "Active".into(),
                root: "/active".into(),
                active: true,
                created_at: 1,
                updated_at: 1,
            },
            tau_proto::projects::Project {
                id: "p_old".into(),
                name: "Old".into(),
                root: "/old".into(),
                active: false,
                created_at: 1,
                updated_at: 1,
            },
        ];
        let state = ProjectState::from_daemon(records, Some("p_active"));
        assert_eq!(state.projects.len(), 2);
        assert_eq!(state.persisted_active_id(), Some("p_active"));
        assert!(state.projects["p_old"].inactive);
        let no_selection = ProjectState::from_daemon(
            vec![tau_proto::projects::Project {
                id: "p_old".into(),
                name: "Old".into(),
                root: "/old".into(),
                active: false,
                created_at: 1,
                updated_at: 1,
            }],
            Some("p_old"),
        );
        assert!(no_selection.persisted_active_id().is_none());
    }
}
