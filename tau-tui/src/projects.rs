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
    EmptyName,
    EmptyRoot,
    AlreadyExists(ProjectId),
    NotFound(ProjectId),
    InactiveConflict(ProjectId),
    NotInactive(ProjectId),
    NoPendingConflict,
}

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
                let id = ProjectId(root.to_owned());
                if let Some(existing) = self.projects.get(&id) {
                    if existing.status == ProjectStatus::Inactive {
                        self.inactive_conflict = Some(InactiveConflict {
                            project: id,
                            name: name.to_owned(),
                            root: root.to_owned(),
                        });
                        return Err(ProjectError::InactiveConflict(ProjectId(root.to_owned())));
                    }
                    return Err(ProjectError::AlreadyExists(id));
                }
                self.insert_active(id.clone(), name, root);
                self.selected = Some(id);
                Ok(())
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
                        let id = self.next_id(&conflict.root);
                        self.insert_active(id.clone(), &conflict.name, &conflict.root);
                        self.selected = Some(id);
                        Ok(())
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

    fn insert_active(&mut self, id: ProjectId, name: &str, root: &str) {
        self.projects.insert(
            id.clone(),
            Project {
                id,
                name: name.to_owned(),
                root: root.to_owned(),
                status: ProjectStatus::Active,
            },
        );
    }

    fn next_id(&self, root: &str) -> ProjectId {
        let base = ProjectId(root.to_owned());
        if !self.projects.contains_key(&base) {
            return base;
        }
        (2..)
            .map(|suffix| ProjectId(format!("{root}#{suffix}")))
            .find(|id| !self.projects.contains_key(id))
            .expect("unbounded project id space")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_crud_register_reactivate_and_restore() {
        let mut state = ProjectState::default();
        state
            .apply(ProjectAction::Register {
                name: "demo".into(),
                root: "/tmp/demo".into(),
            })
            .unwrap();
        let id = ProjectId("/tmp/demo".into());
        state.local_restore.insert(id.clone(), "draft".into());
        state.apply(ProjectAction::Delete(id.clone())).unwrap();
        assert_eq!(
            state.apply(ProjectAction::Register {
                name: "demo".into(),
                root: "/tmp/demo".into()
            }),
            Err(ProjectError::InactiveConflict(id.clone()))
        );
        state
            .apply(ProjectAction::ChooseInactiveConflict(
                InactiveConflictChoice::Reactivate,
            ))
            .unwrap();
        state
            .apply(ProjectAction::RestoreLocal(id.clone()))
            .unwrap();
        assert_eq!(state.selected, Some(id));
    }

    #[test]
    fn inactive_registration_can_create_a_new_id_without_changing_root() {
        let mut state = ProjectState::default();
        state
            .apply(ProjectAction::Register {
                name: "old".into(),
                root: "/tmp/shared".into(),
            })
            .unwrap();
        let old = ProjectId("/tmp/shared".into());
        state.apply(ProjectAction::Unregister(old.clone())).unwrap();
        assert!(matches!(
            state.apply(ProjectAction::Register {
                name: "new".into(),
                root: "/tmp/shared".into(),
            }),
            Err(ProjectError::InactiveConflict(_))
        ));
        state
            .apply(ProjectAction::ChooseInactiveConflict(
                InactiveConflictChoice::CreateNew,
            ))
            .unwrap();
        let selected = state.selected.clone().unwrap();
        assert_ne!(selected, old);
        assert_eq!(state.projects[&selected].root, "/tmp/shared");
    }
}
