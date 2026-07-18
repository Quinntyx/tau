//! The project shell: a small, deliberately dumb projection around the projects
//! boundary.  Keeping the projection independent of the daemon makes loading,
//! empty, and failure states reachable before a connection exists.

use std::path::PathBuf;

use gpui::{
    ClickEvent, Context, Entity, IntoElement, Render, SharedString, Window, div, prelude::*, px,
    rgb,
};

use crate::projects::ProjectAction;

gpui::actions!(project_shell, [ToggleProjects, NewProject, OpenSettings]);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectItem {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectState {
    Loading,
    Ready(Vec<ProjectItem>),
    Empty,
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellLayout {
    Full,
    Rail,
    ContentOnly,
}

/// Actions emitted by shell controls.  The parent view can translate these
/// into the project service without making this presentation module own a
/// wire protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellAction {
    ToggleProjects,
    NewProject,
    OpenSettings,
    SelectProject(String),
}

/// The typed boundary used by the shell's production controls.  Keeping this
/// separate from GPUI events means the service can be replaced by a daemon,
/// an in-process registry, or a test double without changing the view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectServiceAction {
    Create {
        name: String,
        root: PathBuf,
    },
    Select(String),
    Update {
        id: String,
        name: String,
        root: PathBuf,
    },
    Unregister(String),
    Reactivate(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InactiveProjectChoice {
    Reactivate,
    CreateNew,
}

pub trait ProjectServiceAdapter {
    fn dispatch(&mut self, action: ProjectServiceAction);
}

pub fn layout_for_width(width: f32) -> ShellLayout {
    if width < 640.0 {
        ShellLayout::ContentOnly
    } else if width < 900.0 {
        ShellLayout::Rail
    } else {
        ShellLayout::Full
    }
}

/// Stable projection used by the view and by focused tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellDescription {
    pub layout: ShellLayout,
    pub state: &'static str,
    pub selected: Option<String>,
    pub project_count: usize,
}

pub struct ProjectShell {
    pub projects: ProjectState,
    pub selected: Option<String>,
    pub projects_visible: bool,
    pub last_action: Option<ShellAction>,
    pub last_service_action: Option<ProjectServiceAction>,
}

impl ProjectShell {
    pub fn new(projects: ProjectState) -> Self {
        Self {
            projects,
            selected: None,
            projects_visible: true,
            last_action: None,
            last_service_action: None,
        }
    }

    pub fn select(&mut self, id: impl Into<String>) {
        let id = id.into();
        self.selected = Some(id.clone());
        self.last_action = Some(ShellAction::SelectProject(id));
        self.last_service_action = self.selected.clone().map(ProjectServiceAction::Select);
    }

    pub fn dispatch_project_action(&mut self, action: ProjectServiceAction) {
        if let ProjectServiceAction::Select(id) = &action {
            self.select(id.clone());
        }
        self.last_service_action = Some(action);
    }

    pub fn dispatch_to<A: ProjectServiceAdapter>(
        &mut self,
        adapter: &mut A,
        action: ProjectServiceAction,
    ) {
        adapter.dispatch(action.clone());
        self.dispatch_project_action(action);
    }

    pub fn create_project(&mut self, name: impl Into<String>, root: PathBuf) {
        self.dispatch_project_action(ProjectServiceAction::Create {
            name: name.into(),
            root,
        });
    }

    pub fn update_project(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        root: PathBuf,
    ) {
        self.dispatch_project_action(ProjectServiceAction::Update {
            id: id.into(),
            name: name.into(),
            root,
        });
    }

    pub fn unregister_project(&mut self, id: impl Into<String>) {
        self.dispatch_project_action(ProjectServiceAction::Unregister(id.into()));
    }

    pub fn register_inactive(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        root: PathBuf,
        choice: InactiveProjectChoice,
    ) {
        match choice {
            InactiveProjectChoice::Reactivate => {
                self.dispatch_project_action(ProjectServiceAction::Reactivate(id.into()))
            }
            InactiveProjectChoice::CreateNew => self.create_project(name, root),
        }
    }

    pub fn apply_project_action(&mut self, action: ProjectAction) {
        if let ProjectAction::Select(id) = &action {
            self.selected = Some(id.clone());
        }
        self.last_action = Some(match action {
            ProjectAction::Select(id) => {
                self.last_service_action = Some(ProjectServiceAction::Select(id.clone()));
                ShellAction::SelectProject(id)
            }
            ProjectAction::Create { .. }
            | ProjectAction::Update { .. }
            | ProjectAction::Unregister(_)
            | ProjectAction::Reactivate(_) => {
                self.last_service_action = Some(match action {
                    ProjectAction::Create { name, root } => {
                        ProjectServiceAction::Create { name, root }
                    }
                    ProjectAction::Update { id, name, root } => {
                        ProjectServiceAction::Update { id, name, root }
                    }
                    ProjectAction::Unregister(id) => ProjectServiceAction::Unregister(id),
                    ProjectAction::Reactivate(id) => ProjectServiceAction::Reactivate(id),
                    ProjectAction::Select(_) => unreachable!(),
                });
                ShellAction::ToggleProjects
            }
        });
    }

    fn new_project(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.last_action = Some(ShellAction::NewProject);
        cx.notify();
    }

    fn open_settings(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.last_action = Some(ShellAction::OpenSettings);
        cx.notify();
    }

    pub fn describe(&self, width: f32) -> ShellDescription {
        let (state, count) = match &self.projects {
            ProjectState::Loading => ("loading", 0),
            ProjectState::Ready(items) => ("ready", items.len()),
            ProjectState::Empty => ("empty", 0),
            ProjectState::Error(_) => ("error", 0),
        };
        ShellDescription {
            layout: layout_for_width(width),
            state,
            selected: self.selected.clone(),
            project_count: count,
        }
    }

    fn project_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.projects {
            ProjectState::Loading => div().child("Loading projects…"),
            ProjectState::Empty => div().child("No projects yet"),
            ProjectState::Error(message) => {
                div().child(format!("Could not load projects: {message}"))
            }
            ProjectState::Ready(items) => items.iter().fold(div(), |list, project| {
                let id = project.id.clone();
                let selected = self.selected.as_deref() == Some(project.id.as_str());
                list.child(
                    div()
                        .id(SharedString::from(project.id.clone()))
                        .px_2()
                        .py_2()
                        .rounded_md()
                        .bg(if selected {
                            rgb(0x26354a)
                        } else {
                            rgb(0x151a21)
                        })
                        .child(project.name.clone())
                        .on_click(cx.listener(move |shell, _, _, _| shell.select(id.clone()))),
                )
            }),
        };
        div().flex().flex_col().gap_2().p_3().child(body)
    }
}

impl Render for ProjectShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let layout = layout_for_width(f32::from(window.viewport_size().width));
        let rail = div()
            .w(px(52.))
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .p_2()
            .child("τ")
            .child(
                div()
                    .id("new-project")
                    .child("+")
                    .on_click(cx.listener(Self::new_project)),
            )
            .child(
                div()
                    .id("settings")
                    .child("⚙")
                    .on_click(cx.listener(Self::open_settings)),
            );
        let content = div().flex_1().min_w_0().p_6().child(match &self.selected {
            Some(id) => format!("Project {id}"),
            None => "Select a project to begin".to_string(),
        });
        let root = div()
            .size_full()
            .flex()
            .bg(rgb(0x0d1117))
            .text_color(rgb(0xd6deeb));
        match layout {
            ShellLayout::Full => root.child(rail).child(self.project_list(cx)).child(content),
            ShellLayout::Rail => root.child(rail).child(content),
            ShellLayout::ContentOnly => root.child(content),
        }
    }
}

pub fn create_shell(cx: &mut Context<ProjectShell>, state: ProjectState) -> Entity<ProjectShell> {
    cx.new(|_| ProjectShell::new(state))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collapses_progressively() {
        assert_eq!(layout_for_width(1200.), ShellLayout::Full);
        assert_eq!(layout_for_width(800.), ShellLayout::Rail);
        assert_eq!(layout_for_width(500.), ShellLayout::ContentOnly);
    }
    #[test]
    fn states_are_described() {
        let shell = ProjectShell::new(ProjectState::Empty);
        assert_eq!(shell.describe(900.).state, "empty");
    }

    #[test]
    fn project_action_is_reachable_from_shell_boundary() {
        let mut shell = ProjectShell::new(ProjectState::Ready(Vec::new()));
        shell.apply_project_action(ProjectAction::Select("demo".into()));
        assert_eq!(shell.selected.as_deref(), Some("demo"));
        assert_eq!(
            shell.last_action,
            Some(ShellAction::SelectProject("demo".into()))
        );
    }

    #[test]
    fn service_controls_reach_typed_actions() {
        let mut shell = ProjectShell::new(ProjectState::Ready(Vec::new()));
        shell.create_project("demo", PathBuf::from("/tmp/demo"));
        assert_eq!(
            shell.last_service_action,
            Some(ProjectServiceAction::Create {
                name: "demo".into(),
                root: PathBuf::from("/tmp/demo")
            })
        );
        shell.update_project("demo", "renamed", PathBuf::from("/tmp/new"));
        assert!(matches!(
            shell.last_service_action,
            Some(ProjectServiceAction::Update { .. })
        ));
        shell.unregister_project("demo");
        assert_eq!(
            shell.last_service_action,
            Some(ProjectServiceAction::Unregister("demo".into()))
        );
        shell.register_inactive(
            "old",
            "old",
            PathBuf::from("/tmp/old"),
            InactiveProjectChoice::Reactivate,
        );
        assert_eq!(
            shell.last_service_action,
            Some(ProjectServiceAction::Reactivate("old".into()))
        );
        shell.register_inactive(
            "old",
            "new",
            PathBuf::from("/tmp/new"),
            InactiveProjectChoice::CreateNew,
        );
        assert!(matches!(
            shell.last_service_action,
            Some(ProjectServiceAction::Create { .. })
        ));
    }
}
