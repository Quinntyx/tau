//! Native gpui client for tau model turns.

pub mod backend;
pub mod chat;
pub mod feed;
pub mod input;
pub mod picker;
pub mod preferences;
pub mod projects;
pub mod sessions;
pub mod shell;
pub mod view;

use std::path::PathBuf;

use anyhow::{Context, Result};
use backend::Backend;
use gpui::{
    App, AppContext, Application, Bounds, Context as GpuiContext, Entity, IntoElement,
    ParentElement, Render, Styled, Window, WindowBounds, WindowOptions, div, px, size,
};
use projects::{ProjectAction, ProjectError, ProjectState};
use shell::{
    InactiveProjectChoice, ProjectItem, ProjectServiceAction, ProjectServiceAdapter, ProjectShell,
};
use tokio::runtime::{Handle, Runtime};
use view::TauView;

/// The production root keeps the typed project registry at the shell boundary.
/// Backend actions remain owned by `TauView`; the shell never speaks a wire
/// protocol and can therefore expose loading/empty/error states independently.
pub struct ProjectShellRoot {
    shell: Entity<ProjectShell>,
    view: Entity<TauView>,
    pub project_adapter: LocalProjectAdapter,
}

/// Local implementation of the S1 adapter seam. It keeps this slice
/// production-wirable before the project RPC types exist on the branch.
#[derive(Debug, Default)]
pub struct LocalProjectAdapter {
    pub state: ProjectState,
    pub last_error: Option<ProjectError>,
}

impl ProjectServiceAdapter for LocalProjectAdapter {
    fn dispatch(&mut self, action: ProjectServiceAction) {
        self.last_error = None;
        let action = match action {
            ProjectServiceAction::Create { name, root } => ProjectAction::Create { name, root },
            ProjectServiceAction::Select(id) => ProjectAction::Select(id),
            ProjectServiceAction::Update { id, name, root } => {
                ProjectAction::Update { id, name, root }
            }
            ProjectServiceAction::Unregister(id) => ProjectAction::Unregister(id),
            ProjectServiceAction::Reactivate(id) => ProjectAction::Reactivate(id),
        };
        if let Err(error) = self.state.apply(action) {
            self.last_error = Some(error);
        }
    }
}

impl ProjectShellRoot {
    fn new(backend: Backend, cx: &mut GpuiContext<Self>) -> Self {
        let cwd = PathBuf::from(backend.cwd());
        let mut state = ProjectState::new();
        let id = state
            .create("Current project".into(), cwd.clone())
            .expect("fresh project");
        let shell_state = shell::ProjectState::Ready(vec![ProjectItem {
            id: id.clone(),
            name: "Current project".into(),
            path: cwd,
        }]);
        let shell = cx.new(|_: &mut GpuiContext<ProjectShell>| {
            let mut project_shell = ProjectShell::new(shell_state.clone());
            project_shell.select(id);
            project_shell
        });
        let view = cx.new(|cx| TauView::new(backend, cx));
        Self {
            shell,
            view,
            project_adapter: LocalProjectAdapter {
                state,
                last_error: None,
            },
        }
    }

    /// Narrow adapter seam for menus, persistence, and future project dialogs.
    pub fn apply_project_action(&mut self, action: ProjectAction, cx: &mut GpuiContext<Self>) {
        let service_action = match &action {
            ProjectAction::Create { name, root } => ProjectServiceAction::Create {
                name: name.clone(),
                root: root.clone(),
            },
            ProjectAction::Select(id) => ProjectServiceAction::Select(id.clone()),
            ProjectAction::Update { id, name, root } => ProjectServiceAction::Update {
                id: id.clone(),
                name: name.clone(),
                root: root.clone(),
            },
            ProjectAction::Unregister(id) => ProjectServiceAction::Unregister(id.clone()),
            ProjectAction::Reactivate(id) => ProjectServiceAction::Reactivate(id.clone()),
        };
        self.apply_service_action(service_action, cx);
    }

    /// Dispatch a shell action through the typed adapter and refresh the
    /// visible projection only after the local adapter accepts it.
    pub fn apply_service_action(
        &mut self,
        action: ProjectServiceAction,
        cx: &mut GpuiContext<Self>,
    ) {
        self.project_adapter.dispatch(action.clone());
        let error = self
            .project_adapter
            .last_error
            .as_ref()
            .map(|error| format!("{error:?}"));
        let selected = self.project_adapter.state.active_project_id.clone();
        let items = project_items(&self.project_adapter.state);
        self.shell.update(cx, |shell, cx| {
            shell.dispatch_project_action(action);
            if let Some(error) = error {
                shell.projects = shell::ProjectState::Error(error);
            } else {
                shell.projects = shell::ProjectState::Ready(items);
            }
            shell.selected = selected;
            cx.notify();
        });
    }

    /// Registering an inactive path deliberately requires an explicit choice.
    pub fn register_inactive(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        root: PathBuf,
        choice: InactiveProjectChoice,
        cx: &mut GpuiContext<Self>,
    ) {
        match choice {
            InactiveProjectChoice::Reactivate => {
                self.apply_service_action(ProjectServiceAction::Reactivate(id.into()), cx)
            }
            InactiveProjectChoice::CreateNew => self.apply_service_action(
                ProjectServiceAction::Create {
                    name: name.into(),
                    root,
                },
                cx,
            ),
        }
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

fn project_items(state: &ProjectState) -> Vec<ProjectItem> {
    state
        .projects
        .values()
        .map(|project| ProjectItem {
            id: project.id.clone(),
            name: if project.inactive {
                format!("{} (inactive)", project.name)
            } else {
                project.name.clone()
            },
            path: project.root.clone(),
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
        Backend::from_parts_with_startup(socket, handle, daemon, auto_started, cwd, model);
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
    fn local_adapter_reaches_project_lifecycle_without_wire_types() {
        let mut adapter = LocalProjectAdapter::default();
        adapter.dispatch(ProjectServiceAction::Create {
            name: "demo".into(),
            root: "/tmp/demo".into(),
        });
        let id = adapter.state.active_project_id.clone().unwrap();
        adapter.dispatch(ProjectServiceAction::Update {
            id: id.clone(),
            name: "renamed".into(),
            root: "/tmp/renamed".into(),
        });
        adapter.dispatch(ProjectServiceAction::Select(id.clone()));
        adapter.dispatch(ProjectServiceAction::Unregister(id.clone()));
        adapter.dispatch(ProjectServiceAction::Reactivate(id));
        assert!(adapter.last_error.is_none());
        assert!(adapter.state.active().is_some());
    }
}
