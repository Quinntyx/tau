//! Client-local project registry.
//!
//! This module deliberately contains no transport or server API.  A project is
//! a GUI concern: the registry remembers workspace groups and which one the
//! client last displayed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub type ProjectId = String;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub root: PathBuf,
    pub inactive: bool,
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
}

impl ProjectState {
    pub fn new() -> Self {
        Self::default()
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

    /// Return the daemon identity and canonical workspace root for the active
    /// project.  Keeping this pair together prevents callers from falling back
    /// to the GUI process working directory when dispatching operations.
    pub fn active_selection(&self) -> Option<(&ProjectId, &Path)> {
        self.active()
            .map(|project| (&project.id, project.root.as_path()))
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
        if self
            .projects
            .values()
            .any(|p| p.root == root && !p.inactive)
        {
            return Err(ProjectError::AlreadyRegistered(root));
        }
        let id = fresh_id(&self.projects, &root);
        self.projects.insert(
            id.clone(),
            Project {
                id: id.clone(),
                name,
                root,
                inactive: false,
            },
        );
        self.active_project_id = Some(id.clone());
        Ok(id)
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
            .find(|p| p.root == root && p.inactive)
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

fn fresh_id(projects: &BTreeMap<ProjectId, Project>, root: &Path) -> ProjectId {
    let base = format!("project-{:016x}", stable_hash(root));
    if !projects.contains_key(&base) {
        return base;
    }
    (1..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !projects.contains_key(id))
        .unwrap()
}

fn stable_hash(path: &Path) -> u64 {
    path.to_string_lossy()
        .bytes()
        .fold(14695981039346656037, |h, b| {
            (h ^ b as u64).wrapping_mul(1099511628211)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scripted_lifecycle_and_restore() {
        let mut state = ProjectState::new();
        let id = state.create("one".into(), PathBuf::from("/one")).unwrap();
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
        let old = state
            .create("old".into(), PathBuf::from("/shared"))
            .unwrap();
        state.unregister(&old).unwrap();
        let new = state
            .register(
                "new".into(),
                PathBuf::from("/shared"),
                InactiveRegistration::CreateNew,
            )
            .unwrap();
        assert_ne!(new, old);
        assert_eq!(state.projects[&new].root, PathBuf::from("/shared"));
        assert_eq!(state.active_project_id.as_deref(), Some(new.as_str()));
    }

    #[test]
    fn active_selection_exposes_identity_and_canonical_root() {
        let mut state = ProjectState::default();
        let id = state
            .create("workspace".into(), PathBuf::from("/canonical/root"))
            .unwrap();
        let (identity, root) = state.active_selection().unwrap();
        assert_eq!(identity, &id);
        assert_eq!(root, Path::new("/canonical/root"));
    }
}
