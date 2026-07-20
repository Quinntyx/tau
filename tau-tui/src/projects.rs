//! Typed daemon-backed project shell state.
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
    /// The daemon's active flag, retained separately for wire fidelity.
    pub active: bool,
}

impl From<tau_proto::projects::Project> for Project {
    fn from(value: tau_proto::projects::Project) -> Self {
        Self {
            id: ProjectId(value.id),
            name: value.name,
            root: value.root,
            status: if value.active {
                ProjectStatus::Active
            } else {
                ProjectStatus::Inactive
            },
            active: value.active,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CwdProject {
    Registered(ProjectId),
    Unresolved(String),
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
    /// Replace the projection with records acknowledged by the daemon.
    pub fn replace_projects<I>(&mut self, projects: I)
    where
        I: IntoIterator<Item = tau_proto::projects::Project>,
    {
        self.projects = projects
            .into_iter()
            .map(Project::from)
            .map(|project| (project.id.clone(), project))
            .collect();
        if self
            .selected
            .as_ref()
            .is_some_and(|id| self.projects.get(id).is_none_or(|project| !project.active))
        {
            self.selected = None;
        }
        // Conflicts are UI intents against a previous daemon projection and
        // must not survive an authoritative refresh.
        self.inactive_conflict = None;
    }

    /// Match a cwd against daemon roots without registering or inventing an id.
    pub fn matching_cwd(&self, cwd: &str) -> CwdProject {
        self.projects
            .values()
            .find(|p| p.root == cwd && p.active)
            .map(|p| CwdProject::Registered(p.id.clone()))
            .unwrap_or_else(|| CwdProject::Unresolved(cwd.to_owned()))
    }

    /// Return the daemon record for a cwd, including inactive records.  This
    /// is used only to present the explicit reactivate/new-ID choice; it never
    /// turns an unregistered cwd into a local project.
    pub fn cwd_record(&self, cwd: &str) -> Option<&Project> {
        self.projects.values().find(|project| project.root == cwd)
    }

    pub fn ordered_ids(&self) -> Vec<ProjectId> {
        self.projects.keys().cloned().collect()
    }

    pub fn select_active(&mut self, id: Option<&ProjectId>) -> Option<&Project> {
        self.selected = id.and_then(|id| {
            self.projects
                .get(id)
                .filter(|p| p.active)
                .map(|p| p.id.clone())
        });
        self.selected.as_ref().and_then(|id| self.projects.get(id))
    }

    /// Restore by daemon id first, then by its acknowledged root.
    pub fn restore_selection(
        &mut self,
        id: Option<&ProjectId>,
        root: Option<&str>,
    ) -> Option<&Project> {
        let candidate = id
            .and_then(|id| self.projects.get(id).filter(|p| p.active))
            .or_else(|| {
                root.and_then(|root| self.projects.values().find(|p| p.root == root && p.active))
            });
        self.selected = candidate.map(|p| p.id.clone());
        self.selected
            .as_ref()
            .and_then(|selected| self.projects.get(selected))
    }

    pub fn restore_active(&mut self, saved: Option<&ProjectId>) -> Option<&Project> {
        self.selected = saved.and_then(|id| {
            self.projects
                .get(id)
                .filter(|project| project.active)
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
                    if !existing.active {
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
                project.active = false;
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
                if project.active {
                    return Err(ProjectError::NotInactive(id));
                }
                project.status = ProjectStatus::Active;
                project.active = true;
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
                Some(project) if project.active => {
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
                active: true,
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

    #[test]
    fn daemon_active_flag_is_authoritative_for_selection_and_conflicts() {
        let id = ProjectId("/tmp/daemon".into());
        let mut state = ProjectState {
            projects: [(
                id.clone(),
                Project {
                    id: id.clone(),
                    name: "daemon".into(),
                    root: "/tmp/daemon".into(),
                    // Deliberately disagree: `active` is the wire value.
                    status: ProjectStatus::Inactive,
                    active: true,
                },
            )]
            .into_iter()
            .collect(),
            ..ProjectState::default()
        };

        state
            .apply(ProjectAction::Select(Some(id.clone())))
            .unwrap();
        assert_eq!(state.selected, Some(id.clone()));
        assert_eq!(
            state.apply(ProjectAction::Reactivate(id)),
            Err(ProjectError::NotInactive(ProjectId("/tmp/daemon".into())))
        );
    }

    #[test]
    fn ordering_and_cwd_matching_are_deterministic() {
        let mut state = ProjectState::default();
        state.replace_projects([
            tau_proto::projects::Project {
                id: "z".into(),
                name: "z".into(),
                root: "/tmp/work".into(),
                active: false,
                created_at: 0,
                updated_at: 0,
            },
            tau_proto::projects::Project {
                id: "a".into(),
                name: "a".into(),
                root: "/tmp/work".into(),
                active: true,
                created_at: 0,
                updated_at: 0,
            },
        ]);
        assert_eq!(
            state.ordered_ids(),
            [ProjectId("a".into()), ProjectId("z".into())]
        );
        assert_eq!(
            state.matching_cwd("/tmp/work"),
            CwdProject::Registered(ProjectId("a".into()))
        );
        assert_eq!(
            state.matching_cwd("/tmp/missing"),
            CwdProject::Unresolved("/tmp/missing".into())
        );
    }
}
