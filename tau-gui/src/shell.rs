//! The project shell: a small, deliberately dumb projection around the projects
//! boundary.  Keeping the projection independent of the daemon makes loading,
//! empty, and failure states reachable before a connection exists.

use std::path::PathBuf;

use gpui::{
    ClickEvent, Context, Entity, IntoElement, Render, SharedString, Window, div, prelude::*, px,
    rgb,
};

use crate::projects::ProjectAction;
use crate::theme::Theme;

gpui::actions!(project_shell, [ToggleProjects, NewProject]);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectItem {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
}

impl ProjectItem {
    /// Project-list projection for a daemon-backed domain record. Inactive
    /// records remain visible for explicit Reactivate/New ID actions.
    pub fn from_project(project: &crate::projects::Project) -> Self {
        Self {
            id: project.id.clone(),
            name: if project.inactive {
                format!("{} (inactive)", project.name)
            } else {
                project.name.clone()
            },
            path: project.root.clone(),
        }
    }
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
    RetryProjects,
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
    CreateNewId(String),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectPrompt {
    pub name: String,
    pub root: PathBuf,
    pub open: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleStatus {
    Idle,
    Loading,
    Error(String),
    Acknowledged(String),
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
    pub create_prompt: Option<CreateProjectPrompt>,
    pub lifecycle: LifecycleStatus,
    pub inactive_choice: Option<(String, String, PathBuf)>,
    pub menu_open_for: Option<String>,
}

impl ProjectShell {
    pub fn new(projects: ProjectState) -> Self {
        Self {
            projects,
            selected: None,
            projects_visible: true,
            last_action: None,
            last_service_action: None,
            create_prompt: None,
            lifecycle: LifecycleStatus::Idle,
            inactive_choice: None,
            menu_open_for: None,
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

    /// Open the real create form, prefilling its name from the selected path.
    pub fn open_create_prompt(&mut self, root: PathBuf) {
        let name = root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project")
            .to_owned();
        self.create_prompt = Some(CreateProjectPrompt {
            name,
            root,
            open: true,
        });
        self.last_action = Some(ShellAction::NewProject);
    }

    pub fn set_create_fields(&mut self, name: impl Into<String>, root: PathBuf) {
        if let Some(prompt) = self.create_prompt.as_mut() {
            prompt.name = name.into();
            prompt.root = root;
        }
    }

    pub fn submit_create_prompt(&mut self) -> Option<ProjectServiceAction> {
        let prompt = self.create_prompt.take()?;
        if prompt.name.trim().is_empty() || prompt.root.as_os_str().is_empty() {
            self.create_prompt = Some(prompt);
            return None;
        }
        let action = ProjectServiceAction::Create {
            name: prompt.name,
            root: prompt.root,
        };
        self.lifecycle = LifecycleStatus::Loading;
        self.dispatch_project_action(action.clone());
        Some(action)
    }

    pub fn cancel_create_prompt(&mut self) {
        self.create_prompt = None;
        self.last_action = Some(ShellAction::ToggleProjects);
    }

    pub fn set_lifecycle(&mut self, status: LifecycleStatus) {
        self.lifecycle = status;
    }

    /// Ask the owning root to reload the authoritative project projection.
    /// The shell deliberately does not perform the load itself.
    pub fn retry_projects(&mut self) {
        self.last_action = Some(ShellAction::RetryProjects);
        self.lifecycle = LifecycleStatus::Loading;
    }

    pub fn choose_inactive(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        root: PathBuf,
    ) {
        self.inactive_choice = Some((id.into(), name.into(), root));
    }

    pub fn submit_inactive_choice(
        &mut self,
        choice: InactiveProjectChoice,
    ) -> Option<ProjectServiceAction> {
        let (id, _name, _root) = self.inactive_choice.take()?;
        let action = match choice {
            InactiveProjectChoice::Reactivate => ProjectServiceAction::Reactivate(id),
            InactiveProjectChoice::CreateNew => ProjectServiceAction::CreateNewId(id),
        };
        self.lifecycle = LifecycleStatus::Loading;
        self.dispatch_project_action(action.clone());
        Some(action)
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
            InactiveProjectChoice::CreateNew => {
                let _ = (name, root);
                self.dispatch_project_action(ProjectServiceAction::CreateNewId(id.into()));
            }
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
        self.open_create_prompt(PathBuf::from("project"));
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

    fn project_list(&self, cx: &mut Context<Self>, theme: Theme) -> gpui::Div {
        let body = match &self.projects {
            ProjectState::Loading => div().child("Loading projects…").child(
                div()
                    .id("retry-projects-loading")
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(theme.elevated)
                    .text_color(theme.text)
                    .cursor_pointer()
                    .child("Refresh")
                    .on_click(cx.listener(|shell, _, _, cx| {
                        shell.retry_projects();
                        cx.notify();
                    })),
            ),
            ProjectState::Empty => div().child("No projects yet").child(
                div()
                    .id("create-project-empty")
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(theme.elevated)
                    .text_color(theme.text)
                    .cursor_pointer()
                    .child("Create project")
                    .on_click(cx.listener(|shell, _, _, cx| {
                        shell.open_create_prompt(PathBuf::from("project"));
                        cx.notify();
                    })),
            ),
            ProjectState::Error(message) => div()
                .child(format!("Could not load projects: {message}"))
                .child(
                    div()
                        .id("retry-projects-error")
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(theme.elevated)
                        .text_color(theme.text)
                        .cursor_pointer()
                        .child("Retry")
                        .on_click(cx.listener(|shell, _, _, cx| {
                            shell.retry_projects();
                            cx.notify();
                        })),
                ),
            ProjectState::Ready(items) => items.iter().fold(div(), |list, project| {
                let id = project.id.clone();
                let inactive = project.name.ends_with(" (inactive)");
                let selected = self.selected.as_deref() == Some(project.id.as_str());
                let menu_open = self.menu_open_for.as_deref() == Some(id.as_str());
                let menu_id = id.clone();
                let mut row = div()
                    .id(SharedString::from(project.id.clone()))
                    .debug_selector(|| project.id.clone())
                    .relative()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if selected {
                        theme.selection
                    } else {
                        theme.sidebar
                    })
                    .hover(move |style| {
                        if selected {
                            style
                        } else {
                            style.bg(theme.elevated)
                        }
                    })
                    .child(div().flex_1().child(project.name.clone()))
                    .when(inactive, |r| {
                        r.child(
                            div()
                                .text_xs()
                                .text_color(theme.tertiary_text)
                                .child("inactive"),
                        )
                    });

                let menu_trigger_id = menu_id.clone();
                let debug_trigger_id = menu_trigger_id.clone();
                let menu_trigger = div()
                    .id(SharedString::from(format!(
                        "project-menu-btn-{menu_trigger_id}"
                    )))
                    .debug_selector(move || format!("project-menu-btn-{debug_trigger_id}"))
                    .px_1()
                    .py_0p5()
                    .rounded_md()
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(theme.secondary_text)
                    .hover(|style| style.bg(theme.elevated).text_color(theme.text))
                    .cursor_pointer()
                    .child("⋮")
                    .on_click(cx.listener(move |shell, _, _, cx| {
                        if shell.menu_open_for.as_deref() == Some(menu_trigger_id.as_str()) {
                            shell.menu_open_for = None;
                        } else {
                            shell.menu_open_for = Some(menu_trigger_id.clone());
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }));

                let actions_cluster = div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .when(!menu_open, |div| {
                        div.opacity(0.0).hover(|style| style.opacity(1.0))
                    })
                    .child(menu_trigger);

                if menu_open {
                    let mut dropdown = div()
                        .absolute()
                        .top_full()
                        .right_0()
                        .mt_1()
                        .w(px(140.0))
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .p_1()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.separator)
                        .bg(theme.elevated)
                        .shadow_md();

                    if inactive {
                        let reactivate_id = id.clone();
                        let new_id = id.clone();
                        let name = project.name.trim_end_matches(" (inactive)").to_owned();
                        let reactivate_name = name.clone();
                        let new_name = name;
                        let path = project.path.clone();
                        let reactivate_path = path.clone();
                        dropdown = dropdown
                            .child(
                                div()
                                    .id(SharedString::from(format!("reactivate-{id}")))
                                    .debug_selector(|| format!("reactivate-{id}"))
                                    .px_2()
                                    .py_1()
                                    .text_xs()
                                    .rounded_md()
                                    .text_color(theme.accent)
                                    .hover(|style| style.bg(theme.selection))
                                    .cursor_pointer()
                                    .child("Reactivate")
                                    .on_click(cx.listener(move |shell, _, _, cx| {
                                        shell.choose_inactive(
                                            reactivate_id.clone(),
                                            reactivate_name.clone(),
                                            reactivate_path.clone(),
                                        );
                                        let _ = shell.submit_inactive_choice(
                                            InactiveProjectChoice::Reactivate,
                                        );
                                        shell.menu_open_for = None;
                                        cx.stop_propagation();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("new-id-{new_id}")))
                                    .debug_selector(|| format!("new-id-{new_id}"))
                                    .px_2()
                                    .py_1()
                                    .text_xs()
                                    .rounded_md()
                                    .text_color(theme.secondary_text)
                                    .hover(|style| style.bg(theme.selection))
                                    .cursor_pointer()
                                    .child("New ID")
                                    .on_click(cx.listener(move |shell, _, _, cx| {
                                        shell.choose_inactive(
                                            new_id.clone(),
                                            new_name.clone(),
                                            path.clone(),
                                        );
                                        let _ = shell.submit_inactive_choice(
                                            InactiveProjectChoice::CreateNew,
                                        );
                                        shell.menu_open_for = None;
                                        cx.stop_propagation();
                                        cx.notify();
                                    })),
                            );
                    } else {
                        let update_id = id.clone();
                        let unregister_id = id.clone();
                        let update_debug = update_id.clone();
                        let unregister_debug = unregister_id.clone();
                        let update_name = project.name.clone();
                        let update_path = project.path.clone();
                        dropdown = dropdown
                            .child(
                                div()
                                    .id(SharedString::from(format!("update-{update_id}")))
                                    .debug_selector(move || format!("update-{update_debug}"))
                                    .px_2()
                                    .py_1()
                                    .text_xs()
                                    .rounded_md()
                                    .text_color(theme.text)
                                    .hover(|style| style.bg(theme.selection))
                                    .cursor_pointer()
                                    .child("Update")
                                    .on_click(cx.listener(move |shell, _, _, cx| {
                                        shell.update_project(
                                            update_id.clone(),
                                            update_name.clone(),
                                            update_path.clone(),
                                        );
                                        shell.menu_open_for = None;
                                        cx.stop_propagation();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("unregister-{unregister_id}")))
                                    .debug_selector(move || {
                                        format!("unregister-{unregister_debug}")
                                    })
                                    .px_2()
                                    .py_1()
                                    .text_xs()
                                    .rounded_md()
                                    .text_color(theme.error_text)
                                    .hover(|style| style.bg(theme.selection))
                                    .cursor_pointer()
                                    .child("Unregister")
                                    .on_click(cx.listener(move |shell, _, _, cx| {
                                        shell.unregister_project(unregister_id.clone());
                                        shell.menu_open_for = None;
                                        cx.stop_propagation();
                                        cx.notify();
                                    })),
                            );
                    }
                    row = row
                        .child(actions_cluster)
                        .child(gpui::deferred(dropdown).with_priority(30));
                } else {
                    row = row.child(actions_cluster);
                }
                row = row.on_click(cx.listener(move |shell, _, _, cx| {
                    shell.select(id.clone());
                    cx.notify();
                }));
                list.child(row)
            }),
        };
        let prompt = self.create_prompt.as_ref().map(|prompt| {
            div()
                .id("create-project-prompt")
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.text)
                        .child("Create project"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.secondary_text)
                        .child(format!("Name: {}", prompt.name)),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.secondary_text)
                        .child(format!("Path: {}", prompt.root.display())),
                )
                .child(
                    div()
                        .id("submit-create-project")
                        .debug_selector(|| "submit-create-project".into())
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(theme.accent)
                        .text_color(rgb(0xffffff))
                        .cursor_pointer()
                        .child("Create")
                        .on_click(cx.listener(|shell, _, _, cx| {
                            let _ = shell.submit_create_prompt();
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("cancel-create-project")
                        .debug_selector(|| "cancel-create-project".into())
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(theme.elevated)
                        .text_color(theme.text)
                        .cursor_pointer()
                        .child("Cancel")
                        .on_click(cx.listener(|shell, _, _, cx| {
                            shell.cancel_create_prompt();
                            cx.notify();
                        })),
                )
        });
        let container = div().flex().flex_col().gap_2().p_3().child(body);
        if let Some(prompt) = prompt {
            container.child(prompt)
        } else {
            container
        }
    }
}

impl Render for ProjectShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let layout = layout_for_width(f32::from(window.viewport_size().width));
        let theme = Theme::for_appearance(window.appearance());
        let rail = div()
            .w(px(52.))
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .p_2()
            .bg(theme.toolbar)
            .child(
                div()
                    .size(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_lg()
                    .bg(theme.accent)
                    .text_color(rgb(0xffffff))
                    .child("T"),
            )
            .child(
                div()
                    .id("new-project")
                    .size(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_lg()
                    .bg(theme.elevated)
                    .cursor_pointer()
                    .child("+")
                    .on_click(cx.listener(Self::new_project)),
            );
        let root = div()
            .h_full()
            .flex()
            .flex_none()
            .border_r_1()
            .border_color(theme.separator)
            .bg(theme.sidebar)
            .text_color(theme.text);
        let list = self.project_list(cx, theme);
        let status = match &self.lifecycle {
            LifecycleStatus::Idle => None,
            LifecycleStatus::Loading => Some("Working…".to_string()),
            LifecycleStatus::Error(message) => Some(format!("Error: {message}")),
            LifecycleStatus::Acknowledged(message) => Some(message.clone()),
        };
        let list = if let Some(message) = status {
            list.child(div().pt_2().child(message))
        } else {
            list
        };
        match layout {
            ShellLayout::Full => root.w(px(320.)).child(rail).child(list.flex_1().min_w_0()),
            ShellLayout::Rail => root.w(px(52.)).child(rail),
            ShellLayout::ContentOnly => root.w(px(0.)),
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
    fn loading_and_error_states_expose_a_reload_action() {
        let mut shell = ProjectShell::new(ProjectState::Loading);
        shell.retry_projects();
        assert_eq!(shell.last_action, Some(ShellAction::RetryProjects));
        assert_eq!(shell.lifecycle, LifecycleStatus::Loading);
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
            Some(ProjectServiceAction::CreateNewId(_))
        ));
    }

    #[test]
    fn create_prompt_prefills_and_has_submit_cancel_seams() {
        let mut shell = ProjectShell::new(ProjectState::Empty);
        shell.open_create_prompt(PathBuf::from("/work/demo"));
        assert_eq!(shell.create_prompt.as_ref().unwrap().name, "demo");
        shell.set_create_fields("Demo", PathBuf::from("/work/demo"));
        assert!(matches!(
            shell.submit_create_prompt(),
            Some(ProjectServiceAction::Create { .. })
        ));
        assert!(matches!(shell.lifecycle, LifecycleStatus::Loading));
        shell.open_create_prompt(PathBuf::from("/work/other"));
        shell.cancel_create_prompt();
        assert!(shell.create_prompt.is_none());
    }

    #[test]
    fn inactive_choice_is_explicit_and_ack_hooks_are_projected() {
        let mut shell = ProjectShell::new(ProjectState::Empty);
        shell.choose_inactive("old", "Old", PathBuf::from("/old"));
        assert_eq!(
            shell.submit_inactive_choice(InactiveProjectChoice::Reactivate),
            Some(ProjectServiceAction::Reactivate("old".into()))
        );
        shell.set_lifecycle(LifecycleStatus::Acknowledged("Saved".into()));
        assert!(matches!(shell.lifecycle, LifecycleStatus::Acknowledged(_)));
    }

    #[test]
    fn daemon_projection_keeps_inactive_records_actionable() {
        let project = crate::projects::Project {
            id: "p_old".into(),
            name: "Old".into(),
            root: PathBuf::from("/old"),
            inactive: true,
        };
        let item = ProjectItem::from_project(&project);
        assert_eq!(item.id, "p_old");
        assert_eq!(item.name, "Old (inactive)");
        assert_eq!(item.path, PathBuf::from("/old"));
    }
}
