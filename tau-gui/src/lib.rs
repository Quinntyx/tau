//! Native gpui client for tau model turns.

pub mod backend;
pub mod chat;
pub mod composer;
pub mod feed;
pub mod input;
pub mod operations;
pub mod picker;
pub mod preferences;
pub mod projects;
pub mod sessions;
pub mod shell;
pub mod theme;
pub mod view;

use std::path::PathBuf;

use anyhow::{Context, Result};
use backend::Backend;
use gpui::{
    App, AppContext, Application, Bounds, Context as GpuiContext, Entity, IntoElement,
    ParentElement, Render, Styled, Window, WindowBounds, WindowOptions, div, px, size,
};
use shell::{ProjectItem, ProjectServiceAction, ProjectShell};
use tau_proto::projects::{Project as DaemonProject, ProjectRenameParams, ProjectRepathParams};
use tokio::runtime::{Handle, Runtime};
use view::TauView;

/// The production root keeps the typed project registry at the shell boundary.
/// Backend actions remain owned by `TauView`; the shell never speaks a wire
/// protocol and can therefore expose loading/empty/error states independently.
pub struct ProjectShellRoot {
    shell: Entity<ProjectShell>,
    view: Entity<TauView>,
    backend: Backend,
    projects: Vec<DaemonProject>,
    selected_project_id: Option<String>,
    last_shell_action: Option<ProjectServiceAction>,
    retry_in_flight: bool,
    preferences: crate::preferences::GuiPreferences,
    pub cwd_unregistered: bool,
    pub create_project_name: String,
    pub create_project_root: PathBuf,
}

impl ProjectShellRoot {
    fn new(backend: Backend, cx: &mut GpuiContext<Self>) -> Self {
        let cwd = PathBuf::from(backend.cwd());
        let create_project_name = cwd
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("project")
            .to_owned();
        let shell = cx.new(|_: &mut GpuiContext<ProjectShell>| {
            ProjectShell::new(shell::ProjectState::Loading)
        });
        let view = cx.new(|cx| TauView::new(backend.clone(), cx));
        let mut root = Self {
            shell,
            view,
            backend,
            projects: Vec::new(),
            selected_project_id: None,
            last_shell_action: None,
            retry_in_flight: false,
            preferences: crate::preferences::path()
                .ok()
                .and_then(|path| crate::preferences::GuiPreferences::load_from(&path).ok())
                .unwrap_or_default(),
            cwd_unregistered: true,
            create_project_name,
            create_project_root: cwd,
        };
        cx.observe(&root.shell, |root, shell, cx| {
            root.sync_from_shell(shell, cx);
        })
        .detach();
        root.reload_projects(cx, None);
        root
    }

    fn sync_from_shell(&mut self, shell: Entity<ProjectShell>, cx: &mut GpuiContext<Self>) {
        let shell_state = shell.read(cx);
        let service_action = shell_state.last_service_action.clone();
        let retry_requested = shell_state.last_action == Some(shell::ShellAction::RetryProjects);
        let selected = shell_state.selected.clone();
        if service_action != self.last_shell_action {
            self.last_shell_action = service_action.clone();
            if let Some(action) = service_action {
                self.begin_service_action(action, cx);
            }
        }
        if retry_requested && !self.retry_in_flight {
            self.reload_projects(cx, None);
        }
        if !retry_requested {
            self.retry_in_flight = false;
        }
        if selected != self.selected_project_id {
            self.selected_project_id = selected.clone();
            self.select_view_project(selected, cx);
        }
    }

    fn reload_projects(&mut self, cx: &mut GpuiContext<Self>, preferred: Option<String>) {
        let request = self.backend.project_list(true);
        self.retry_in_flight = true;
        self.shell.update(cx, |shell, cx| {
            shell.projects = shell::ProjectState::Loading;
            shell.set_lifecycle(shell::LifecycleStatus::Loading);
            cx.notify();
        });
        cx.spawn(async move |this, cx| match request.await {
            Ok(Ok(result)) => {
                let _ = this.update(cx, |root, cx| {
                    root.retry_in_flight = false;
                    root.projects = result.projects;
                    let cwd = root.create_project_root.to_string_lossy();
                    root.cwd_unregistered = !root
                        .projects
                        .iter()
                        .any(|project| project.active && project.root == cwd);
                    let selected = preferred
                        .or_else(|| root.preferences.selected_project_id.clone())
                        .and_then(|id| {
                            root.projects
                                .iter()
                                .find(|project| project.id == id && project.active)
                                .map(|project| project.id.clone())
                        })
                        .or_else(|| {
                            root.projects
                                .iter()
                                .find(|project| project.active && project.root == cwd)
                                .map(|project| project.id.clone())
                        });
                    root.selected_project_id = selected.clone();
                    let items = project_items(&root.projects);
                    root.shell.update(cx, |shell, cx| {
                        shell.projects = if items.is_empty() {
                            shell::ProjectState::Empty
                        } else {
                            shell::ProjectState::Ready(items)
                        };
                        shell.selected = selected.clone();
                        shell.set_lifecycle(shell::LifecycleStatus::Acknowledged(
                            "Projects loaded".into(),
                        ));
                        shell.last_action = None;
                        cx.notify();
                    });
                    root.select_view_project(selected, cx);
                    if root.cwd_unregistered {
                        root.shell.update(cx, |shell, cx| {
                            shell.open_create_prompt(root.create_project_root.clone());
                            cx.notify();
                        });
                    }
                    cx.notify();
                });
            }
            Ok(Err(error)) => {
                let _ = this.update(cx, |root, cx| {
                    root.retry_in_flight = false;
                    root.shell.update(cx, |shell, cx| {
                        shell.projects = shell::ProjectState::Error(error.to_string());
                        shell.set_lifecycle(shell::LifecycleStatus::Error(error.to_string()));
                        shell.last_action = None;
                        cx.notify();
                    });
                });
            }
            Err(error) => {
                let _ = this.update(cx, |root, cx| {
                    root.retry_in_flight = false;
                    root.shell.update(cx, |shell, cx| {
                        shell.projects = shell::ProjectState::Error(error.to_string());
                        shell.set_lifecycle(shell::LifecycleStatus::Error(error.to_string()));
                        shell.last_action = None;
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    fn select_view_project(&mut self, id: Option<String>, cx: &mut GpuiContext<Self>) {
        // The shell is only a projection and may emit an id for an archived
        // row.  Never let an inactive/unknown daemon record become the view's
        // active project (or get persisted as the last-active preference).
        let id = id.and_then(|id| {
            self.projects
                .iter()
                .find(|project| project.id == *id && project.active)
                .map(|project| (project.id.clone(), PathBuf::from(&project.root)))
        });
        let (project_id, root) = id
            .map(|(id, root)| (Some(id), Some(root)))
            .unwrap_or((None, None));
        if self.shell.read(cx).selected != project_id {
            let selected = project_id.clone();
            self.shell.update(cx, |shell, cx| {
                shell.selected = selected;
                cx.notify();
            });
        }
        self.view.update(cx, |view, cx| {
            view.select_project(project_id.clone(), root, cx)
        });
        if self.preferences.selected_project_id != project_id {
            self.preferences.selected_project_id = project_id;
            let _ = self.preferences.save();
        }
    }

    fn begin_service_action(&mut self, action: ProjectServiceAction, cx: &mut GpuiContext<Self>) {
        self.last_shell_action = Some(action.clone());
        self.shell.update(cx, |shell, cx| {
            shell.set_lifecycle(shell::LifecycleStatus::Loading);
            cx.notify();
        });
        let backend = self.backend.clone();
        let preferred = match &action {
            ProjectServiceAction::Select(id)
            | ProjectServiceAction::Update { id, .. }
            | ProjectServiceAction::Reactivate(id) => Some(id.clone()),
            ProjectServiceAction::Unregister(_)
            | ProjectServiceAction::Create { .. }
            | ProjectServiceAction::CreateNewId(_) => None,
        };
        cx.spawn(async move |this, cx| {
            let result: Result<Option<String>> = async {
                match action {
                    ProjectServiceAction::Create { name, root } => {
                        let response = backend
                            .project_create(name, root.to_string_lossy().into_owned())
                            .await??;
                        Ok(Some(response.project.id))
                    }
                    ProjectServiceAction::CreateNewId(source_id) => {
                        let response = backend.project_new_id(source_id).await??;
                        Ok(Some(response.project_id))
                    }
                    ProjectServiceAction::Select(id) => Ok(Some(id)),
                    ProjectServiceAction::Update { id, name, root } => {
                        backend
                            .project_rename(ProjectRenameParams {
                                project_id: id.clone(),
                                name,
                            })
                            .await??;
                        backend
                            .project_repath(ProjectRepathParams {
                                project_id: id,
                                root: root.to_string_lossy().into_owned(),
                            })
                            .await??;
                        Ok(None)
                    }
                    ProjectServiceAction::Unregister(id) => {
                        backend.project_unregister(id).await??;
                        Ok(None)
                    }
                    ProjectServiceAction::Reactivate(id) => {
                        let response = backend.project_reactivate(id).await??;
                        Ok(Some(response.project.id))
                    }
                }
            }
            .await;
            let _ = this.update(cx, |root, cx| match result {
                Ok(created_or_selected) => {
                    let preferred = created_or_selected.or(preferred);
                    root.reload_projects(cx, preferred);
                }
                Err(error) => {
                    root.last_shell_action = None;
                    root.shell.update(cx, |shell, cx| {
                        shell.last_service_action = None;
                        shell.set_lifecycle(shell::LifecycleStatus::Error(error.to_string()));
                        cx.notify();
                    });
                }
            });
        })
        .detach();
    }

    /// Service actions originate in the shell, but are acknowledged by the
    /// daemon before the projection is refreshed.
    pub fn apply_service_action(
        &mut self,
        action: ProjectServiceAction,
        cx: &mut GpuiContext<Self>,
    ) {
        self.shell.update(cx, |shell, cx| {
            shell.last_service_action = Some(action.clone());
            cx.notify();
        });
        self.begin_service_action(action, cx);
    }

    /// Production callers use this to surface loading, empty, and transport
    /// errors while S1 data is being fetched or acknowledged.
    pub fn set_project_state(&mut self, state: shell::ProjectState, cx: &mut GpuiContext<Self>) {
        self.shell.update(cx, |shell, cx| {
            shell.projects = state;
            cx.notify();
        });
    }
}

fn project_items(projects: &[DaemonProject]) -> Vec<ProjectItem> {
    projects
        .iter()
        .map(|project| ProjectItem {
            id: project.id.clone(),
            name: if !project.active {
                format!("{} (inactive)", project.name)
            } else {
                project.name.clone()
            },
            path: PathBuf::from(&project.root),
        })
        .collect()
}

impl Render for ProjectShellRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut GpuiContext<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .child(self.shell.clone())
            .child(div().flex_1().min_w_0().child(self.view.clone()))
    }
}

pub fn run(socket: PathBuf) -> Result<()> {
    let owned_runtime = Handle::try_current()
        .is_err()
        .then(Runtime::new)
        .transpose()?;
    let handle = match Handle::try_current() {
        Ok(handle) => handle,
        Err(_) => owned_runtime
            .as_ref()
            .expect("owned runtime")
            .handle()
            .clone(),
    };
    // Keep daemon startup in the normal Result path so a missing or stale socket
    // can be reported without panicking before the recovery UI is available.
    let (daemon, auto_started, cwd, model) = if let Some(runtime) = owned_runtime.as_ref() {
        runtime
            .block_on(Backend::prepare(&socket))
            .context("starting tau daemon")?
    } else {
        tokio::task::block_in_place(|| handle.block_on(Backend::prepare(&socket)))
            .context("starting tau daemon")?
    };
    let backend =
        Backend::from_parts_with_startup(socket, handle.clone(), daemon, auto_started, cwd, model);
    Application::new().run(move |cx: &mut App| {
        input::bind_keys(cx);
        view::bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1_100.), px(760.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some("tau".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                move |_, cx| cx.new(|cx| ProjectShellRoot::new(backend, cx)),
            )
            .context("opening tau window");
        if let Err(error) = window {
            eprintln!("{error:#}");
            return;
        }
        cx.activate(true);
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_project_records_project_to_shell_items() {
        let records = vec![
            DaemonProject {
                id: "active".into(),
                name: "Active".into(),
                root: "/tmp/active".into(),
                active: true,
                created_at: 0,
                updated_at: 0,
            },
            DaemonProject {
                id: "inactive".into(),
                name: "Old".into(),
                root: "/tmp/old".into(),
                active: false,
                created_at: 0,
                updated_at: 0,
            },
        ];
        let items = project_items(&records);
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].name, "Old (inactive)");
    }
}
