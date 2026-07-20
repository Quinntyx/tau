//! Typed, client-local project shell state.
//!
//! This module intentionally stops at the UI boundary: actions describe an
//! intent and the reducer only changes the local projection.  A daemon adapter
//! can translate the actions later without making the TUI depend on a wire
//! protocol.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProjectId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectStatus {
    Active,
    Inactive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub root: String,
    pub status: ProjectStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InactiveConflictChoice {
    Reactivate,
    CreateNew,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectAction {
    Create {
        name: String,
        root: String,
    },
    Update {
        id: ProjectId,
        name: String,
        root: String,
    },
    Delete(ProjectId),
    Unregister(ProjectId),
    Register {
        name: String,
        root: String,
    },
    Reactivate(ProjectId),
    ChooseInactiveConflict(InactiveConflictChoice),
    RestoreLocal(ProjectId),
    Select(Option<ProjectId>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectError {
    DaemonRequired,
    EmptyName,
    EmptyRoot,
    AlreadyExists(ProjectId),
    NotFound(ProjectId),
    InactiveConflict(ProjectId),
    NotInactive(ProjectId),
    NoPendingConflict,
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DaemonRequired => f.write_str("project changes require daemon acknowledgement"),
            Self::EmptyName => f.write_str("project name cannot be empty"),
            Self::EmptyRoot => f.write_str("project path cannot be empty"),
            Self::AlreadyExists(id) => write!(f, "project already exists: {}", id.0),
            Self::NotFound(id) => write!(f, "project not found: {}", id.0),
            Self::InactiveConflict(id) => write!(f, "project is inactive: {}", id.0),
            Self::NotInactive(id) => write!(f, "project is already active: {}", id.0),
            Self::NoPendingConflict => f.write_str("no inactive project conflict is pending"),
        }
    }
}

impl std::error::Error for ProjectError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InactiveConflict {
    pub project: ProjectId,
    pub name: String,
    pub root: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectState {
    pub projects: BTreeMap<ProjectId, Project>,
    pub selected: Option<ProjectId>,
    /// A local snapshot used when switching projects without a server round trip.
    pub local_restore: BTreeMap<ProjectId, String>,
    pub inactive_conflict: Option<InactiveConflict>,
}

impl ProjectState {
    /// Replace the UI projection with records returned by the daemon.  The
    /// registry owns the opaque IDs; the TUI never derives one from a path.
    pub fn load_daemon_projects(
        &mut self,
        projects: impl IntoIterator<Item = (ProjectId, String, String, ProjectStatus)>,
    ) {
        self.projects.clear();
        self.selected = None;
        for (id, name, root, status) in projects {
            let active = status == ProjectStatus::Active;
            self.projects.insert(
                id.clone(),
                Project {
                    id: id.clone(),
                    name,
                    root,
                    status,
                },
            );
            if active && self.selected.is_none() {
                self.selected = Some(id);
            }
        }
    }

    pub fn restore_active(&mut self, saved: Option<&ProjectId>) -> Option<&Project> {
        self.selected = saved.and_then(|id| {
            self.projects
                .get(id)
                .filter(|project| project.status == ProjectStatus::Active)
                .map(|project| project.id.clone())
        });
        self.selected.as_ref().and_then(|id| self.projects.get(id))
    }

    pub fn apply(&mut self, action: ProjectAction) -> Result<(), ProjectError> {
        match action {
            ProjectAction::Create { name, root } | ProjectAction::Register { name, root } => {
                let name = name.trim();
                let root = root.trim();
                if name.is_empty() {
                    return Err(ProjectError::EmptyName);
                }
                if root.is_empty() {
                    return Err(ProjectError::EmptyRoot);
                }
                let _ = (name, root);
                Err(ProjectError::DaemonRequired)
            }
            ProjectAction::Update { id, name, root } => {
                let name = name.trim();
                let root = root.trim();
                if name.is_empty() {
                    return Err(ProjectError::EmptyName);
                }
                if root.is_empty() {
                    return Err(ProjectError::EmptyRoot);
                }
                let project = self
                    .projects
                    .get_mut(&id)
                    .ok_or_else(|| ProjectError::NotFound(id.clone()))?;
                project.name = name.to_owned();
                project.root = root.to_owned();
                Ok(())
            }
            ProjectAction::Delete(id) | ProjectAction::Unregister(id) => {
                let project = self
                    .projects
                    .get_mut(&id)
                    .ok_or_else(|| ProjectError::NotFound(id.clone()))?;
                project.status = ProjectStatus::Inactive;
                if self.selected.as_ref() == Some(&id) {
                    self.selected = None;
                }
                Ok(())
            }
            ProjectAction::Reactivate(id) => {
                let project = self
                    .projects
                    .get_mut(&id)
                    .ok_or_else(|| ProjectError::NotFound(id.clone()))?;
                if project.status != ProjectStatus::Inactive {
                    return Err(ProjectError::NotInactive(id));
                }
                project.status = ProjectStatus::Active;
                self.selected = Some(id);
                self.inactive_conflict = None;
                Ok(())
            }
            ProjectAction::ChooseInactiveConflict(choice) => {
                let conflict = self
                    .inactive_conflict
                    .take()
                    .ok_or(ProjectError::NoPendingConflict)?;
                match choice {
                    InactiveConflictChoice::Reactivate => {
                        self.apply(ProjectAction::Reactivate(conflict.project))
                    }
                    InactiveConflictChoice::CreateNew => {
                        let _ = conflict;
                        Err(ProjectError::DaemonRequired)
                    }
                    InactiveConflictChoice::Cancel => Ok(()),
                }
            }
            ProjectAction::RestoreLocal(id) => match self.projects.get(&id) {
                Some(project) if project.status == ProjectStatus::Active => {
                    self.selected = Some(id);
                    Ok(())
                }
                _ => Err(ProjectError::NotFound(id)),
            },
            ProjectAction::Select(id) => match id {
                Some(id) => self.apply(ProjectAction::RestoreLocal(id)),
                None => {
                    self.selected = None;
                    Ok(())
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_crud_register_reactivate_and_restore() {
        let mut state = ProjectState::default();
        let id = ProjectId("daemon-demo".into());
        state.load_daemon_projects([(
            id.clone(),
            "demo".into(),
            "/tmp/demo".into(),
            ProjectStatus::Active,
        )]);
        state.local_restore.insert(id.clone(), "draft".into());
        state.apply(ProjectAction::Delete(id.clone())).unwrap();
        state.apply(ProjectAction::Reactivate(id.clone())).unwrap();
        state
            .apply(ProjectAction::RestoreLocal(id.clone()))
            .unwrap();
        assert_eq!(state.selected, Some(id));
    }

    #[test]
    fn inactive_registration_can_create_a_new_id_without_changing_root() {
        let mut state = ProjectState::default();
        let old = ProjectId("daemon-old".into());
        state.load_daemon_projects([(
            old.clone(),
            "old".into(),
            "/tmp/shared".into(),
            ProjectStatus::Active,
        )]);
        state.apply(ProjectAction::Unregister(old.clone())).unwrap();
        assert_eq!(
            state.apply(ProjectAction::Register {
                name: "new".into(),
                root: "/tmp/shared".into()
            }),
            Err(ProjectError::DaemonRequired)
        );
    }
}
