use gpui::{
    App, Context, Entity, Focusable, KeyBinding, MouseButton, MouseUpEvent, Render, ScrollHandle,
    StatefulInteractiveElement, Task, Window, div, prelude::*, px, rgb,
};
use tau_client::ProjectId;
use tau_client::TurnStreamEvent;
use tau_proto::prelude::{ClientResponse, QuestionAnswer, TurnPermissionChoice};

use crate::backend::{Backend, DaemonAction, DaemonStatus};
use crate::chat::{Card, ChatAction, ChatState, ChatStatus, EventKind, PermissionChoice, Role};
use crate::input::{TextInput, command_mode};
use crate::picker::{
    AgentOption, ModelOption, PickerAction, command_action, command_suggestions, model_groups,
    next_agent, parse_command,
};
use crate::sessions::{
    Navigator, ProjectId as NavigatorProjectId, SessionSummary as NavigatorSession,
};

gpui::actions!(
    tau_view,
    [
        Submit,
        SwitchAgent,
        CycleModel,
        DismissPicker,
        PickerUp,
        PickerDown,
        PickItem,
        ToggleSidebar,
        ToggleFollow,
        ToggleSessions
    ]
);

/// Stable, testable description of the information presented by the view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TauViewDescription {
    pub transcript: Vec<(String, String)>,
    pub status: String,
    pub loading: bool,
    pub sidebar: Vec<(String, String)>,
}

pub fn next_sidebar_visibility(visible: bool) -> bool {
    !visible
}

pub fn next_follow_state(following: bool) -> bool {
    !following
}

/// Build presentation data without requiring a GPUI window.  Typed protocol
/// events are included even when they do not have a legacy card.
pub fn describe_chat(chat: &ChatState, agent: &str, model: &str, cwd: &str) -> TauViewDescription {
    let mut transcript = chat.cards.iter().map(card_description).collect::<Vec<_>>();
    for event in &chat.events {
        let item = match &event.kind {
            EventKind::Started => Some(("lifecycle".into(), "Turn started".into())),
            EventKind::TextDelta { .. } => None,
            EventKind::ReasoningDelta { text } => Some(("reasoning".into(), text.clone())),
            EventKind::ToolOutput {
                inline,
                truncated,
                artifacts,
            } => Some((
                "tool output".into(),
                format!(
                    "{}{}{}",
                    inline,
                    if *truncated { " (truncated)" } else { "" },
                    if artifacts.is_empty() {
                        String::new()
                    } else {
                        format!(" [{} artifacts]", artifacts.len())
                    }
                ),
            )),
            EventKind::PermissionRequested {
                tool, description, ..
            } => Some(("permission".into(), format!("{tool}: {description}"))),
            EventKind::QuestionAsked { question, .. } => {
                Some(("question".into(), question.clone()))
            }
            EventKind::DiffRequested { path, diff, .. } => {
                Some(("diff".into(), format!("{path}\n{diff}")))
            }
            EventKind::ArtifactCreated {
                artifact_id,
                media_type,
                size_bytes,
                storage_ref,
                ..
            } => Some((
                "artifact".into(),
                format!("{artifact_id} ({media_type}, {size_bytes} bytes)\n{storage_ref}"),
            )),
            EventKind::Completed { message_id } => Some((
                "lifecycle".into(),
                format!(
                    "Turn completed{}",
                    message_id
                        .map(|id| format!(" (message {id})"))
                        .unwrap_or_default()
                ),
            )),
            EventKind::Cancelled => Some(("lifecycle".into(), "Turn cancelled".into())),
            EventKind::Failed { message } => Some(("error".into(), message.clone())),
            EventKind::ToolStarted {
                call_id,
                tool,
                input,
            } => Some((
                "tool started".into(),
                format!(
                    "{tool} [{call_id}]{}",
                    input
                        .as_deref()
                        .map(|value| format!("\ninput: {value}"))
                        .unwrap_or_default()
                ),
            )),
            EventKind::ToolStatus {
                call_id,
                status,
                metadata,
            } => Some((
                "tool status".into(),
                format!(
                    "{call_id}: {status:?}{}",
                    metadata
                        .as_deref()
                        .map(|value| format!("\n{value}"))
                        .unwrap_or_default()
                ),
            )),
            EventKind::ToolCompleted { call_id, output } => Some((
                "tool completed".into(),
                format!(
                    "{call_id}{}",
                    output
                        .as_deref()
                        .map(|value| format!("\n{value}"))
                        .unwrap_or_default()
                ),
            )),
            EventKind::ToolError { call_id, error } => {
                Some(("tool error".into(), format!("{call_id}: {error}")))
            }
            EventKind::CompactionStarted => {
                Some(("compaction".into(), "Compaction started".into()))
            }
            EventKind::CompactionCompleted { summary } => Some((
                "compaction".into(),
                summary
                    .clone()
                    .unwrap_or_else(|| "Compaction completed".into()),
            )),
            EventKind::SystemMessage { message } => Some(("system".into(), message.clone())),
            EventKind::IntegrationEvent {
                integration,
                event,
                data,
            } => Some((
                "integration".into(),
                format!(
                    "{integration}: {event}{}",
                    data.as_deref()
                        .map(|value| format!("\n{value}"))
                        .unwrap_or_default()
                ),
            )),
            EventKind::PlanUpdated { plan } => Some(("plan".into(), plan.clone())),
            EventKind::StatusChanged { status, message } => Some((
                "status".into(),
                format!(
                    "{status}{}",
                    message
                        .as_deref()
                        .map(|value| format!(": {value}"))
                        .unwrap_or_default()
                ),
            )),
            EventKind::Telemetry { usage, context } => Some((
                "telemetry".into(),
                format!(
                    "usage={} context={}",
                    usage.as_deref().unwrap_or("unavailable"),
                    context.as_deref().unwrap_or("unavailable")
                ),
            )),
            EventKind::Other => Some(("event".into(), "Unknown typed event".into())),
        };
        if let Some(item) = item {
            transcript.push(item);
        }
    }
    let status = match &chat.status {
        ChatStatus::Ready => "Ready".into(),
        ChatStatus::Streaming => "Thinking...".into(),
        ChatStatus::Failed(error) => format!("Error: {error}"),
    };
    let sidebar = &chat.sidebar;
    let plan = match (&sidebar.plan, &sidebar.current_step, sidebar.airtight) {
        (Some(plan), Some(step), Some(airtight)) => format!(
            "{plan} · {step} ({})",
            if airtight { "airtight" } else { "not airtight" }
        ),
        (Some(plan), _, _) => plan.clone(),
        _ => "Unavailable".into(),
    };
    let tokens = match (
        sidebar.input_tokens,
        sidebar.output_tokens,
        sidebar.context_tokens,
    ) {
        (Some(input), Some(output), Some(context)) => format!(
            "in {input} · out {output}{} · ctx {context}{}",
            sidebar
                .cached_tokens
                .map(|cached| format!(" · cached {cached}"))
                .unwrap_or_default(),
            sidebar
                .context_percent
                .map(|percent| format!(" ({percent}%)"))
                .unwrap_or_default()
        ),
        _ => sidebar
            .total_tokens
            .or(chat.usage)
            .map(|value| format!("{value} total"))
            .unwrap_or_else(|| "Unavailable".into()),
    };
    TauViewDescription {
        transcript,
        status,
        loading: chat.loading || chat.active_assistant.is_some(),
        sidebar: vec![
            (
                "AGENT".into(),
                sidebar.agent.as_deref().unwrap_or(agent).into(),
            ),
            (
                "MODEL".into(),
                sidebar.model.as_deref().unwrap_or(model).into(),
            ),
            ("PLAN".into(), plan),
            (
                "LSP".into(),
                sidebar.lsp.as_deref().unwrap_or("Unavailable").into(),
            ),
            ("TOKENS".into(), tokens),
            (
                "MCP".into(),
                sidebar.mcp.as_deref().unwrap_or("Unavailable").into(),
            ),
            (
                "TASK".into(),
                sidebar
                    .task_tier
                    .map(|tier| format!("Tier {tier}"))
                    .unwrap_or_else(|| "Unavailable".into()),
            ),
            (
                "AUTONOMY".into(),
                sidebar
                    .autonomous
                    .map(|enabled| if enabled { "Enabled" } else { "Disabled" })
                    .unwrap_or("Unavailable")
                    .into(),
            ),
            (
                "SESSION".into(),
                sidebar
                    .session_id
                    .clone()
                    .or_else(|| chat.session_id.clone())
                    .clone()
                    .unwrap_or_else(|| "Unavailable".into()),
            ),
            (
                "DIRECTORY".into(),
                if let Some(value) = sidebar.cwd.as_deref() {
                    value.into()
                } else if cwd.is_empty() {
                    "Unavailable".into()
                } else {
                    cwd.into()
                },
            ),
            (
                "ROOTS".into(),
                if sidebar.roots.is_empty() {
                    "Unavailable".into()
                } else {
                    sidebar.roots.join("\n")
                },
            ),
        ],
    }
}

fn card_description(card: &Card) -> (String, String) {
    match card {
        Card::Message { role, text } => (
            match role {
                Role::User => "You",
                Role::Assistant => "tau",
                Role::System => "system",
                Role::Tool => "tool",
            }
            .into(),
            text.clone(),
        ),
        Card::Tool {
            name,
            input,
            output,
            expanded,
        } => (
            "tool".into(),
            if *expanded {
                format!("{name}\ninput: {input}\noutput: {output}")
            } else {
                format!("{name} (click to inspect)")
            },
        ),
        Card::Diff {
            path,
            patch,
            approved,
            ..
        } => (
            "diff".into(),
            format!(
                "{path} ({})\n{patch}",
                if *approved { "approved" } else { "pending" }
            ),
        ),
        Card::Permission {
            tool, description, ..
        } => ("permission".into(), format!("{tool}: {description}")),
        Card::Question {
            question, answer, ..
        } => (
            "question".into(),
            format!(
                "{question}\n{}",
                answer.as_deref().unwrap_or("awaiting answer")
            ),
        ),
    }
}

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", Submit, None),
        KeyBinding::new("tab", SwitchAgent, None),
        KeyBinding::new("escape", DismissPicker, None),
        KeyBinding::new("up", PickerUp, None),
        KeyBinding::new("down", PickerDown, None),
        KeyBinding::new("enter", PickItem, Some("Picker")),
    ]);
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RuntimeState {
    NotNegotiated,
    Negotiating,
    Ready,
    Failed(String),
}

pub struct TauView {
    input: Entity<TextInput>,
    picker_input: Entity<TextInput>,
    backend: Backend,
    chat: ChatState,
    task: Option<Task<()>>,
    ack_tasks: Vec<Task<()>>,
    agent: String,
    toast_visible: bool,
    model_index: usize,
    models: Vec<String>,
    sidebar_visible: bool,
    runtime: RuntimeState,
    picker: Option<PickerKind>,
    picker_index: usize,
    agents: Vec<AgentOption>,
    preferences: crate::preferences::GuiPreferences,
    transcript_scroll: ScrollHandle,
    follow_output: bool,
    selected_project_id: Option<String>,
    sessions: Navigator,
    session_query: Entity<TextInput>,
    sessions_open: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PickerKind {
    Model,
    Agent,
    Command,
}

impl TauView {
    pub fn new(backend: Backend, cx: &mut Context<Self>) -> Self {
        let input = cx.new(TextInput::new);
        let picker_input = cx.new(TextInput::new);
        let session_query = cx.new(TextInput::new);
        let toast_visible = backend.auto_started();
        let preferences = crate::preferences::path()
            .ok()
            .and_then(|path| crate::preferences::GuiPreferences::load_from(&path).ok())
            .unwrap_or_default();
        // Preferences annotate the catalog; they are not the catalog.  Always
        // retain the configured backend model and every configured preference
        // entry so a recent/favorite flag cannot hide otherwise valid models.
        let mut models = vec![backend.model().to_owned()];
        for model in preferences
            .favorites
            .iter()
            .chain(preferences.recent_models.iter())
            .chain(preferences.selected_model.iter())
        {
            if !models.iter().any(|candidate| candidate == model) {
                models.push(model.clone());
            }
        }
        let selected_model = preferences.selected_model.clone();
        let selected_agent = preferences.selected_agent.clone();
        let default_model = backend.model().to_owned();
        let models = models;
        let model_index = selected_model
            .as_ref()
            .and_then(|model| models.iter().position(|candidate| candidate == model))
            .unwrap_or(0);
        let agent = selected_agent.unwrap_or_default();
        let mut agents = Backend::configured_agents();
        if !agent.is_empty() && !agents.iter().any(|candidate| candidate.name == agent) {
            agents.push(AgentOption {
                name: agent.clone(),
                in_tab_cycle: true,
            });
        }
        let view = Self {
            input,
            picker_input,
            backend,
            chat: ChatState::default(),
            task: None,
            ack_tasks: Vec::new(),
            agent: agent.clone(),
            toast_visible,
            model_index,
            models: if models.is_empty() {
                vec![selected_model.unwrap_or(default_model)]
            } else {
                models
            },
            sidebar_visible: preferences.sidebar,
            runtime: RuntimeState::NotNegotiated,
            picker: None,
            picker_index: 0,
            agents,
            preferences,
            transcript_scroll: ScrollHandle::new(),
            follow_output: true,
            selected_project_id: None,
            sessions: Navigator::default(),
            session_query,
            sessions_open: false,
        };
        // Session loading begins only after an explicit project selection.
        view
    }

    /// Set the active project selected by the project navigator.  Turns are
    /// intentionally rejected until this explicit selection is supplied.
    pub fn select_project(&mut self, project_id: Option<String>) {
        self.selected_project_id = project_id.filter(|id| !id.trim().is_empty());
    }

    fn submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        self.send(window, cx);
    }

    fn click_send(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.send(window, cx);
    }

    /// Create through the daemon before changing the active chat.  This is
    /// deliberately asynchronous so a failed create cannot leave a phantom
    /// local session selected.
    fn new_chat(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let Some(project_id) = self.selected_project_id.clone() else {
            self.runtime = RuntimeState::Failed("select a project before creating a chat".into());
            cx.notify();
            return;
        };
        let project = ProjectId::new(project_id);
        self.ack_tasks.push(cx.spawn(async move |this, cx| {
            let result = match backend.session_create(project).await {
                Ok(task) => task.ok(),
                Err(_) => None,
            };
            if let Some(summary) = result.as_ref() {
                let _ = backend
                    .save_active_session(
                        summary.project_id.clone(),
                        summary.session_id.as_str().to_owned(),
                    )
                    .await
                    .ok()
                    .and_then(|task| task.ok());
            }
            let _ = this.update(cx, |view, cx| {
                if let Some(summary) = result {
                    view.chat.session_id = Some(summary.session_id.as_str().to_owned());
                    view.chat.cards.clear();
                    view.chat.events.clear();
                    view.chat.status = ChatStatus::Ready;
                    view.load_sessions(cx);
                    cx.notify();
                }
            });
        }));
    }

    fn session_project(&self) -> Option<ProjectId> {
        self.selected_project_id.clone().map(ProjectId::new)
    }

    fn load_sessions(&mut self, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let Some(project) = self.session_project() else {
            self.sessions.set_project(None);
            return;
        };
        let navigator_project = NavigatorProjectId::new(project.as_str());
        self.sessions.set_project(Some(navigator_project));
        let include_archived = self.sessions.show_archived();
        self.ack_tasks.push(cx.spawn(async move |this, cx| {
            let listed = backend
                .session_list(project.clone(), include_archived)
                .await
                .ok()
                .and_then(|task| task.ok());
            let restored = backend
                .restore_active_session(project)
                .await
                .ok()
                .and_then(|task| task.ok())
                .flatten();
            let _ = this.update(cx, |view, cx| {
                if let Some(records) = listed {
                    view.sessions
                        .ingest(records.into_iter().map(to_navigator_session));
                }
                if let Some(active) = restored {
                    view.sessions
                        .restore_selection(Some(active.session_id.as_str()));
                    view.chat.session_id = Some(active.session_id.as_str().to_owned());
                }
                cx.notify();
            });
        }));
    }

    fn toggle_sessions(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.sessions_open = !self.sessions_open;
        if self.sessions_open {
            self.load_sessions(cx);
        }
        cx.notify();
    }

    fn select_session(
        &mut self,
        project_id: String,
        session_id: String,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let backend = self.backend.clone();
        let project = ProjectId::new(project_id);
        let reference = tau_client::SessionRef {
            project_id: project.clone(),
            session_id: tau_client::SessionId::new(session_id.clone()),
        };
        self.ack_tasks.push(cx.spawn(async move |this, cx| {
            let history = backend
                .session_history(reference)
                .await
                .ok()
                .and_then(|task| task.ok());
            let _ = backend
                .save_active_session(project, session_id.clone())
                .await
                .ok()
                .and_then(|task| task.ok());
            let _ = this.update(cx, |view, cx| {
                if let Some(history) = history {
                    view.chat.cards.clear();
                    view.chat.events.clear();
                    view.chat.session_id = Some(session_id);
                    for event in history.entries {
                        view.chat.reduce(ChatAction::SessionEvent(event));
                    }
                }
                view.sessions
                    .restore_selection(view.chat.session_id.as_deref());
                view.sessions_open = false;
                cx.notify();
            });
        }));
    }

    fn rename_session(
        &mut self,
        project_id: String,
        session_id: String,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = self.session_query.read(cx).content();
        let title = if title.trim().is_empty() {
            "Untitled".into()
        } else {
            title
        };
        let backend = self.backend.clone();
        self.ack_tasks.push(cx.spawn(async move |this, cx| {
            let _ = backend
                .session_rename(tau_client::SessionRename {
                    project_id: ProjectId::new(project_id),
                    session_id: tau_client::SessionId::new(session_id),
                    title,
                })
                .await
                .ok()
                .and_then(|task| task.ok());
            let _ = this.update(cx, |view, cx| {
                view.load_sessions(cx);
                cx.notify();
            });
        }));
    }

    fn archive_session(
        &mut self,
        project_id: String,
        session_id: String,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session_lifecycle(project_id, session_id, false, cx);
    }

    fn restore_session(
        &mut self,
        project_id: String,
        session_id: String,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session_lifecycle(project_id, session_id, true, cx);
    }

    fn session_lifecycle(
        &mut self,
        project_id: String,
        session_id: String,
        restore: bool,
        cx: &mut Context<Self>,
    ) {
        let backend = self.backend.clone();
        self.ack_tasks.push(cx.spawn(async move |this, cx| {
            let reference = tau_client::SessionRef {
                project_id: ProjectId::new(project_id),
                session_id: tau_client::SessionId::new(session_id),
            };
            let result = if restore {
                backend.session_restore(reference).await
            } else {
                backend.session_archive(reference).await
            };
            let _ = result.ok().and_then(|task| task.ok());
            let _ = this.update(cx, |view, cx| {
                view.load_sessions(cx);
                cx.notify();
            });
        }));
    }

    fn session_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        self.sessions
            .set_query(self.session_query.read(cx).content());
        let rows = self
            .sessions
            .visible()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let show_archived = self.sessions.show_archived();
        div()
            .id("session-navigator")
            .p_3()
            .bg(rgb(0x1b1f27))
            .border_b_1()
            .border_color(rgb(0x2c3340))
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_sm().child("Sessions · newest first"))
            .child(self.session_query.clone())
            .child(toast_button(
                if show_archived {
                    "Hide archived"
                } else {
                    "Show archived"
                },
                0x33445f,
                cx.listener(|view, _, _, cx| {
                    view.sessions
                        .set_show_archived(!view.sessions.show_archived());
                    view.load_sessions(cx);
                    cx.notify();
                }),
            ))
            .children(rows.into_iter().enumerate().map(|(index, record)| {
                let project_id = record.project_id.as_str().to_owned();
                let session_id = record.id.clone();
                let title = record.title.clone().unwrap_or_else(|| "Untitled".into());
                let archived = record.archived;
                let select_project = project_id.clone();
                let select_id = session_id.clone();
                let rename_project = project_id.clone();
                let rename_id = session_id.clone();
                let archive_project = project_id.clone();
                let archive_id = session_id.clone();
                let lifecycle_button = toast_button(
                    if archived { "Restore" } else { "Archive" },
                    if archived { 0x39734a } else { 0x7d3b3b },
                    cx.listener(move |view, event, window, cx| {
                        if archived {
                            view.restore_session(
                                archive_project.clone(),
                                archive_id.clone(),
                                event,
                                window,
                                cx,
                            );
                        } else {
                            view.archive_session(
                                archive_project.clone(),
                                archive_id.clone(),
                                event,
                                window,
                                cx,
                            );
                        }
                    }),
                );
                let mut row = div()
                    .id(("session", index))
                    .p_2()
                    .bg(rgb(
                        if self.sessions.selected() == Some(session_id.as_str()) {
                            0x293b52
                        } else {
                            0x202630
                        },
                    ))
                    .child(format!("{}  {}", title, session_id))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |view, event, window, cx| {
                            view.select_session(
                                select_project.clone(),
                                select_id.clone(),
                                event,
                                window,
                                cx,
                            )
                        }),
                    );
                row = row.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(toast_button(
                            "Rename",
                            0x52627a,
                            cx.listener(move |view, event, window, cx| {
                                view.rename_session(
                                    rename_project.clone(),
                                    rename_id.clone(),
                                    event,
                                    window,
                                    cx,
                                )
                            }),
                        ))
                        .child(lifecycle_button),
                );
                row
            }))
    }

    fn switch_agent(&mut self, _: &SwitchAgent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(next) = next_agent(&self.agents, Some(&self.agent), false) {
            self.select_agent(next);
        }
        self.chat.status = ChatStatus::Ready;
        cx.notify();
    }

    fn cycle_model(&mut self, _: &CycleModel, window: &mut Window, cx: &mut Context<Self>) {
        self.open_picker(PickerKind::Model, window, cx);
        if !self.models.is_empty() {
            self.model_index = (self.model_index + 1) % self.models.len();
        }
        cx.notify();
    }

    fn open_picker(&mut self, kind: PickerKind, window: &mut Window, cx: &mut Context<Self>) {
        self.picker = Some(kind);
        self.picker_index = 0;
        self.picker_input.update(cx, |input, _| input.reset());
        if kind != PickerKind::Command {
            window.focus(&self.picker_input.focus_handle(cx));
        }
    }
    fn dismiss_picker(&mut self, _: &DismissPicker, _: &mut Window, cx: &mut Context<Self>) {
        if self.picker.is_some() {
            self.picker = None;
            self.picker_input.update(cx, |input, _| input.reset());
        } else if command_mode(&self.input.read(cx).content()) {
            self.input.update(cx, |input, _| input.reset());
        }
        cx.notify();
    }
    fn picker_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        let query = if self.picker == Some(PickerKind::Command) || self.picker.is_none() {
            self.input.read(cx).content()
        } else {
            self.picker_input.read(cx).content()
        };
        let count = self.picker_items(&query).len();
        if count > 0 {
            self.picker_index =
                (self.picker_index as isize + delta).rem_euclid(count as isize) as usize;
            cx.notify();
        }
    }
    fn picker_up(&mut self, _: &PickerUp, _: &mut Window, cx: &mut Context<Self>) {
        self.picker_move(-1, cx);
    }
    fn picker_down(&mut self, _: &PickerDown, _: &mut Window, cx: &mut Context<Self>) {
        self.picker_move(1, cx);
    }
    fn picker_items(&self, query: &str) -> Vec<String> {
        self.picker_items_for(self.picker.unwrap_or(PickerKind::Command), query)
    }

    fn picker_items_for(&self, kind: PickerKind, query: &str) -> Vec<String> {
        match kind {
            PickerKind::Model => {
                let options = self.model_options();
                crate::picker::fuzzy_models(&options, query)
                    .into_iter()
                    .map(|model| model.id)
                    .collect()
            }
            PickerKind::Agent => self
                .agents
                .iter()
                .filter(|a| {
                    query.is_empty() || a.name.to_lowercase().contains(&query.to_lowercase())
                })
                .map(|a| a.name.clone())
                .collect(),
            PickerKind::Command => command_suggestions(query, &self.agents, &self.model_options()),
        }
    }
    fn pick_item(&mut self, _: &PickItem, _: &mut Window, cx: &mut Context<Self>) {
        let kind = self.picker.unwrap_or(PickerKind::Command);
        let query = if kind == PickerKind::Command {
            self.input.read(cx).content()
        } else {
            self.picker_input.read(cx).content()
        };
        let items = self.picker_items_for(kind, &query);
        if let Some(item) = items.get(self.picker_index).cloned() {
            match kind {
                PickerKind::Model => self.select_model(item),
                PickerKind::Agent => self.select_agent(item),
                PickerKind::Command => self.accept_command(item, &query, cx),
            }
        }
        self.picker = None;
        self.picker_input.update(cx, |input, _| input.reset());
        cx.notify();
    }
    fn click_model(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.open_picker(PickerKind::Model, window, cx);
        cx.notify();
    }
    fn click_agent(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.open_picker(PickerKind::Agent, window, cx);
        cx.notify();
    }

    fn model_options(&self) -> Vec<ModelOption> {
        self.models
            .iter()
            .map(|id| {
                ModelOption::from_id(
                    id,
                    self.preferences.recent_models.contains(id),
                    self.preferences.favorites.contains(id),
                )
            })
            .collect()
    }

    fn accept_command(&mut self, item: String, query: &str, cx: &mut Context<Self>) {
        let value = if item.starts_with('/') {
            format!("{item} ")
        } else if query.trim_end() == query {
            let start = query
                .rfind(char::is_whitespace)
                .map_or(0, |index| index + 1);
            format!("{}{}", &query[..start], item)
        } else {
            format!("{query}{item}")
        };
        self.input.update(cx, |input, _| input.set_content(value));
        self.picker = None;
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_visible = next_sidebar_visibility(self.sidebar_visible);
        cx.notify();
    }

    fn toggle_follow(&mut self, _: &ToggleFollow, _: &mut Window, cx: &mut Context<Self>) {
        self.follow_output = next_follow_state(self.follow_output);
        if self.follow_output {
            self.transcript_scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn toggle_tool(
        &mut self,
        index: usize,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.chat.reduce(ChatAction::ToggleTool(index));
        cx.notify();
    }

    fn choose_diff(
        &mut self,
        index: usize,
        approved: bool,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (request_id, path) = self
            .chat
            .cards
            .get(index)
            .and_then(|card| match card {
                Card::Diff {
                    request_id, path, ..
                } => Some((request_id.clone(), path.clone())),
                _ => None,
            })
            .unwrap_or_default();
        self.chat.reduce(ChatAction::ApproveDiff(index, approved));
        let receiver = self.backend.respond(ClientResponse::DiffHunk {
            request_id: request_id.clone(),
            path,
            index: index as u32,
            approved,
        });
        self.watch_policy_ack(request_id, receiver, cx);
        cx.notify();
    }

    fn choose_permission(
        &mut self,
        index: usize,
        choice: PermissionChoice,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let request_id = self.chat.cards.get(index).and_then(|card| match card {
            Card::Permission { request_id, .. } => Some(request_id.clone()),
            _ => None,
        });
        self.chat.reduce(ChatAction::Permission(index, choice));
        if matches!(choice, PermissionChoice::Inspect) {
            cx.notify();
            return;
        }
        let choice = match choice {
            PermissionChoice::AllowOnce => TurnPermissionChoice::AllowOnce,
            PermissionChoice::AllowAlways => TurnPermissionChoice::AllowAlways,
            PermissionChoice::Reject => TurnPermissionChoice::Reject,
            PermissionChoice::Inspect => TurnPermissionChoice::Inspect,
            PermissionChoice::Cancel => TurnPermissionChoice::Cancel,
        };
        let request_id = request_id.unwrap_or_default();
        let receiver = self.backend.respond(ClientResponse::Permission {
            request_id: request_id.clone(),
            choice,
        });
        self.watch_policy_ack(request_id, receiver, cx);
        cx.notify();
    }

    fn answer_question(
        &mut self,
        index: usize,
        answer: &'static str,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let question_id = self.chat.cards.get(index).and_then(|card| match card {
            Card::Question { question_id, .. } => Some(question_id.clone()),
            _ => None,
        });
        self.chat
            .reduce(ChatAction::AnswerQuestion(index, answer.into()));
        let question_id = question_id.unwrap_or_default();
        let receiver = self.backend.respond(ClientResponse::Question {
            question_id: question_id.clone(),
            answer: QuestionAnswer(answer.into()),
        });
        self.watch_policy_ack(question_id, receiver, cx);
        cx.notify();
    }

    fn watch_policy_ack(
        &mut self,
        request_id: String,
        receiver: tokio::sync::oneshot::Receiver<Result<(), String>>,
        cx: &mut Context<Self>,
    ) {
        let task = cx.spawn(async move |this, cx| {
            let result = receiver
                .await
                .unwrap_or_else(|_| Err("interactive response task ended".into()));
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(()) => {
                        view.chat.reduce(ChatAction::PolicyAck { request_id });
                    }
                    Err(message) => {
                        view.chat.reduce(ChatAction::PolicyError {
                            request_id,
                            message,
                        });
                    }
                }
                cx.notify();
            });
        });
        self.ack_tasks.push(task);
    }

    fn hide_toast(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.backend.daemon_action(DaemonAction::Okay);
        self.toast_visible = false;
        cx.notify();
    }

    fn never_show_toast(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.backend.daemon_action(DaemonAction::NeverShowAgain);
        self.toast_visible = false;
        cx.notify();
    }

    fn disown_daemon(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.backend.daemon_action(DaemonAction::Disown);
        self.toast_visible = false;
        cx.notify();
    }

    fn always_disown_daemon(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.backend.daemon_action(DaemonAction::AlwaysDisown);
        self.toast_visible = false;
        cx.notify();
    }

    fn quit_gui(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.backend.daemon_action(DaemonAction::Quit);
        cx.quit();
    }

    fn cancel_turn(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.backend.cancel();
        self.chat.active_assistant = None;
        self.chat.status = ChatStatus::Ready;
        self.task = None;
        self.input.update(cx, |input, _| input.set_disabled(false));
        cx.notify();
    }

    fn retry_runtime(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.backend.daemon_action(DaemonAction::Retry);
        self.runtime = RuntimeState::NotNegotiated;
        self.chat.status = ChatStatus::Ready;
        cx.notify();
    }

    fn restart_runtime(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.backend.daemon_action(DaemonAction::Restart);
        self.runtime = RuntimeState::NotNegotiated;
        self.chat.status = ChatStatus::Ready;
        cx.notify();
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chat.active_assistant.is_some() {
            return;
        }
        let prompt = self.input.read(cx).content();
        if prompt.trim().is_empty() {
            return;
        }
        if let Some(command) = parse_command(&prompt) {
            if let Some(action) = command_action(command) {
                match action {
                    PickerAction::OpenModels => self.open_picker(PickerKind::Model, window, cx),
                    PickerAction::OpenAgents => self.open_picker(PickerKind::Agent, window, cx),
                    PickerAction::SelectModel(model) => self.select_model(model),
                    PickerAction::SelectAgent(agent) => self.select_agent(agent),
                    PickerAction::ToggleFavorite(model) => {
                        let _ = self.backend.toggle_favorite(model.clone());
                        self.preferences.toggle_favorite(model);
                    }
                    PickerAction::RecordRecentModel(model) => {
                        let _ = self.backend.record_recent_model(model.clone());
                        self.preferences.record_recent_model(model);
                    }
                    PickerAction::Dismiss => self.picker = None,
                }
                self.input.update(cx, |input, _| input.reset());
                cx.notify();
                return;
            }
        }
        self.input.update(cx, |input, _| {
            input.record_submission(prompt.clone());
            input.reset();
            input.set_disabled(true);
        });
        self.chat.reduce(ChatAction::Submit(prompt.clone()));
        self.runtime = RuntimeState::Negotiating;
        cx.notify();
        window.focus(&self.input.focus_handle(cx));

        let selected_model = self.models.get(self.model_index).cloned();
        let receiver = self.backend.turn_with_project_options(
            prompt,
            self.chat.session_id.clone(),
            (!self.agent.is_empty()).then(|| self.agent.clone()),
            selected_model,
            self.selected_project_id.clone(),
        );
        self.task = Some(cx.spawn(async move |this, cx| {
            let mut receiver = receiver;
            while let Some(event) = receiver.recv().await {
                let done = matches!(event, Ok(TurnStreamEvent::Complete(_)) | Err(_));
                let _ = this.update(cx, |view, cx| {
                    view.apply_event(event, cx);
                });
                if done {
                    break;
                }
            }
        }));
    }

    fn select_model(&mut self, model: String) {
        if let Some(index) = self.models.iter().position(|value| value == &model) {
            self.model_index = index;
        }
        self.preferences.selected_model = Some(model.clone());
        self.preferences.record_recent_model(model);
        let _ = self
            .backend
            .select_model(self.preferences.selected_model.clone().unwrap_or_default());
        let _ = self.preferences.save();
    }

    fn select_agent(&mut self, agent: String) {
        self.agent = agent.clone();
        self.preferences.selected_agent = Some(agent);
        if let Some(selected) = self.preferences.selected_agent.clone() {
            let _ = self.backend.select_agent(selected);
        }
        let _ = self.preferences.save();
    }

    fn picker_overlay(&self, cx: &mut Context<Self>, kind: PickerKind) -> impl IntoElement {
        let title = match kind {
            PickerKind::Model => "Select model",
            PickerKind::Agent => "Select agent",
            PickerKind::Command => "Commands",
        };
        let query = if kind == PickerKind::Command {
            self.input.read(cx).content()
        } else {
            self.picker_input.read(cx).content()
        };
        let items = self.picker_items_for(kind, &query);
        let rows: Vec<(String, Option<usize>)> = match kind {
            PickerKind::Model => {
                let mut rows = Vec::new();
                for (provider, entries) in model_groups(&self.model_options(), &query) {
                    rows.push((format!("{provider} · provider"), None));
                    for model in entries {
                        let index = items.iter().position(|item| item == &model.id);
                        rows.push((
                            format!(
                                "{}{}",
                                if model.favorite {
                                    "★ "
                                } else if model.recent {
                                    "↻ "
                                } else {
                                    ""
                                },
                                model.id
                            ),
                            index,
                        ));
                    }
                }
                rows
            }
            PickerKind::Agent => self
                .agents
                .iter()
                .filter(|a| {
                    query.is_empty() || a.name.to_lowercase().contains(&query.to_lowercase())
                })
                .enumerate()
                .map(|(index, a)| {
                    (
                        format!(
                            "{}{}",
                            a.name,
                            if a.in_tab_cycle { "  · Tab cycle" } else { "" }
                        ),
                        Some(index),
                    )
                })
                .collect(),
            PickerKind::Command => items
                .iter()
                .cloned()
                .enumerate()
                .map(|(index, item)| (item, Some(index)))
                .collect(),
        };
        let search = if kind == PickerKind::Command {
            div()
                .mt_2()
                .p_2()
                .bg(rgb(0x11151b))
                .child(format!("Command: {}", query))
        } else {
            div().mt_2().child(self.picker_input.clone())
        };
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(0x66000000))
            .child(
                div()
                    .key_context("Picker")
                    .w(px(560.))
                    .max_h(px(520.))
                    .p_4()
                    .rounded_lg()
                    .bg(rgb(0x202630))
                    .border_1()
                    .border_color(rgb(0x52627a))
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(title),
                    )
                    .child(search)
                    .children(rows.into_iter().enumerate().map(
                        |(display_index, (row, item_index))| {
                            let selected = item_index == Some(self.picker_index);
                            let favorite_model = (kind == PickerKind::Model)
                                .then(|| item_index.and_then(|index| items.get(index).cloned()))
                                .flatten();
                            let mut row_view = div()
                                .id(("picker-row", display_index))
                                .mt_1()
                                .p_2()
                                .rounded_md()
                                .when(selected, |d| d.bg(rgb(0x34506f)))
                                .when(item_index.is_some(), |d| {
                                    d.cursor_pointer().on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(move |view, _, window, cx| {
                                            view.picker_index = item_index.unwrap_or(0);
                                            view.pick_item(&PickItem, window, cx);
                                        }),
                                    )
                                })
                                .child(row);
                            if let Some(model) = favorite_model {
                                let favorite = self.preferences.favorites.contains(&model);
                                row_view = row_view.child(
                                    div()
                                        .ml_auto()
                                        .cursor_pointer()
                                        .on_mouse_up(
                                            MouseButton::Left,
                                            cx.listener(move |view, _, _, cx| {
                                                let _ = view.backend.toggle_favorite(model.clone());
                                                view.preferences.toggle_favorite(model.clone());
                                                let _ = view.preferences.save();
                                                cx.notify();
                                            }),
                                        )
                                        .child(if favorite { "★" } else { "☆" }),
                                );
                            }
                            row_view
                        },
                    ))
                    .child(
                        div()
                            .mt_3()
                            .text_xs()
                            .text_color(rgb(0x8994a8))
                            .child("↑↓ navigate  Enter select  Esc close"),
                    ),
            )
    }

    fn apply_event(&mut self, event: Result<TurnStreamEvent, String>, cx: &mut Context<Self>) {
        match event {
            Ok(TurnStreamEvent::Event(event)) => {
                self.runtime = RuntimeState::Ready;
                self.chat.reduce(ChatAction::SessionEvent(event));
                cx.notify();
            }
            Ok(TurnStreamEvent::Complete(_)) => {
                self.runtime = RuntimeState::Ready;
                self.chat.active_assistant = None;
                self.chat.status = ChatStatus::Ready;
                self.input.update(cx, |input, _| input.set_disabled(false));
                cx.notify();
            }
            Err(error) => {
                self.runtime = RuntimeState::Failed(error.clone());
                self.chat.reduce(ChatAction::Error(error));
                self.input.update(cx, |input, _| input.set_disabled(false));
                cx.notify();
            }
        }
    }
}

impl Focusable for TauView {
    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for TauView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current_model = self
            .models
            .get(self.model_index)
            .map(String::as_str)
            .unwrap_or(self.backend.model());
        let description = describe_chat(&self.chat, &self.agent, current_model, self.backend.cwd());
        if self.follow_output {
            self.transcript_scroll.scroll_to_bottom();
        }
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .p_4()
            .border_b_1()
            .border_color(rgb(0x2c3340))
            .child(
                div()
                    .text_xl()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("tau"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(toast_button(
                        "New Chat",
                        0x39734a,
                        cx.listener(Self::new_chat),
                    ))
                    .child(toast_button(
                        if self.sessions_open {
                            "Hide sessions"
                        } else {
                            "Sessions"
                        },
                        0x52627a,
                        cx.listener(Self::toggle_sessions),
                    ))
                    .child(toast_button(
                        if self.sidebar_visible {
                            "Hide sidebar"
                        } else {
                            "Show sidebar"
                        },
                        0x33445f,
                        cx.listener(|view, _, _, cx| {
                            view.sidebar_visible = next_sidebar_visibility(view.sidebar_visible);
                            cx.notify();
                        }),
                    ))
                    .child(toast_button(
                        if self.follow_output {
                            "Following"
                        } else {
                            "Follow output"
                        },
                        0x33445f,
                        cx.listener(|view, _, _, cx| {
                            view.follow_output = next_follow_state(view.follow_output);
                            if view.follow_output {
                                view.transcript_scroll.scroll_to_bottom();
                            }
                            cx.notify();
                        }),
                    ))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x8c96a8))
                            .cursor_pointer()
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::click_model))
                            .child(format!(
                                "{}  ·  {}",
                                current_model,
                                match self.backend.daemon_status() {
                                    DaemonStatus::Absent => "Absent",
                                    DaemonStatus::Spawning => "Spawning",
                                    DaemonStatus::Connecting => "Connecting",
                                    DaemonStatus::Negotiating => "Negotiating",
                                    DaemonStatus::Incompatible => "Incompatible",
                                    DaemonStatus::Degraded => "Degraded",
                                    DaemonStatus::Ready => match &self.chat.status {
                                        ChatStatus::Ready => "Ready",
                                        ChatStatus::Streaming => "Thinking...",
                                        ChatStatus::Failed(_) => "Request failed",
                                    },
                                    DaemonStatus::Failed => "Failed",
                                }
                            )),
                    ),
            );
        let runtime_banner = match &self.runtime {
            RuntimeState::NotNegotiated => Some((
                "Runtime is not negotiated. Send a message to begin protocol negotiation.",
                false,
            )),
            RuntimeState::Negotiating => Some((
                "Negotiating runtime capabilities and protocol; tau is not ready yet.",
                false,
            )),
            RuntimeState::Ready => None,
            RuntimeState::Failed(error) => Some((
                if error.is_empty() {
                    "Runtime negotiation failed."
                } else {
                    error.as_str()
                },
                true,
            )),
        };
        let runtime_banner_view = runtime_banner.map(|(message, failed)| {
            let mut banner = div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .p_3()
                .bg(rgb(if failed { 0x4a2929 } else { 0x202b38 }))
                .text_color(rgb(if failed { 0xffb4a8 } else { 0xb9d7ff }))
                .child(div().flex_1().child(message.to_string()));
            if failed {
                banner = banner.child(toast_button(
                    "Retry",
                    0x52627a,
                    cx.listener(Self::retry_runtime),
                ));
                banner = banner.child(toast_button(
                    "Restart owned daemon",
                    0x6a5834,
                    cx.listener(Self::restart_runtime),
                ));
            }
            banner
                .child(toast_button(
                    "Disown safely",
                    0x6a5834,
                    cx.listener(Self::disown_daemon),
                ))
                .child(toast_button("Quit", 0x7d3b3b, cx.listener(Self::quit_gui)))
        });
        let toast = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .p_3()
            .bg(rgb(0x463b24))
            .text_color(rgb(0xf3d28a))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child("tau started the daemon automatically for this GUI session")
                    .child(div().text_xs().child(
                        "The daemon will remain owned by this window unless you disown it.",
                    )),
            )
            .child(
                div()
                    .flex()
                    .id("startup-actions")
                    .max_h(px(420.))
                    .overflow_y_scroll()
                    .gap_2()
                    .child(toast_button(
                        "Okay",
                        0xd8aa4e,
                        cx.listener(Self::hide_toast),
                    ))
                    .child(toast_button(
                        "Don't show again",
                        0x6a5834,
                        cx.listener(Self::never_show_toast),
                    ))
                    .child(toast_button(
                        "Disown",
                        0x6a5834,
                        cx.listener(Self::disown_daemon),
                    ))
                    .child(toast_button(
                        "Always disown",
                        0x6a5834,
                        cx.listener(Self::always_disown_daemon),
                    ))
                    .child(toast_button("Quit", 0x7d3b3b, cx.listener(Self::quit_gui))),
            );
        let transcript = div()
            .id("transcript")
            .track_scroll(&self.transcript_scroll)
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_3()
            .p_5()
            .when(self.chat.cards.is_empty(), |view| {
                view.child(div().text_sm().text_color(rgb(0x8994a8)).child(
                    if self.chat.active_assistant.is_some() {
                        "Loading transcript…"
                    } else {
                        "No transcript yet"
                    },
                ))
            })
            .children(self.chat.cards.iter().enumerate().map(|(index, card)| {
                let (background, label, align_end, text) = match card {
                    Card::Message {
                        role: Role::User,
                        text,
                    } => (0x274d72, "You", true, text.clone()),
                    Card::Message {
                        role: Role::Assistant,
                        text,
                    } => (0x202630, "tau", false, text.clone()),
                    Card::Message {
                        role: Role::System,
                        text,
                    } => (0x202630, "system", false, text.clone()),
                    Card::Message {
                        role: Role::Tool,
                        text,
                    } => (0x3b3425, "tool", false, text.clone()),
                    Card::Tool {
                        name,
                        input,
                        output,
                        expanded,
                    } => (
                        0x3b3425,
                        "tool",
                        false,
                        if *expanded {
                            format!("{name}\ninput: {input}\noutput: {output}")
                        } else {
                            format!("{name} (click to inspect)")
                        },
                    ),
                    Card::Diff {
                        path,
                        patch,
                        approved,
                        ..
                    } => (
                        0x293b32,
                        "diff",
                        false,
                        format!(
                            "{path} ({})\n{patch}",
                            if *approved { "approved" } else { "pending" }
                        ),
                    ),
                    Card::Permission {
                        tool, description, ..
                    } => (
                        0x463b24,
                        "permission",
                        false,
                        format!("{tool}: {description}"),
                    ),
                    Card::Question {
                        question, answer, ..
                    } => (
                        0x293b32,
                        "question",
                        false,
                        format!(
                            "{question}\n{}",
                            answer.as_deref().unwrap_or("awaiting answer")
                        ),
                    ),
                };
                let mut card_view = div()
                    .id(("card", index))
                    .focusable()
                    .hover(|style| style.bg(rgb(0x171d26)))
                    .flex()
                    .flex_col()
                    .when(align_end, |element| element.items_end())
                    .child(div().text_xs().text_color(rgb(0x8994a8)).child(label))
                    .child(
                        div()
                            .id(("card-content", index))
                            .flex()
                            .max_w(px(820.))
                            .max_h(px(360.))
                            .overflow_y_scroll()
                            .p_3()
                            .rounded_lg()
                            .bg(rgb(background))
                            .child(text),
                    );
                if matches!(card, Card::Tool { .. }) {
                    card_view = card_view.on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |view, event, window, cx| {
                            view.toggle_tool(index, event, window, cx)
                        }),
                    );
                }
                if matches!(
                    card,
                    Card::Diff {
                        approved: false,
                        ..
                    }
                ) {
                    card_view = card_view.child(
                        div()
                            .flex()
                            .gap_2()
                            .child(toast_button(
                                "Accept",
                                0x39734a,
                                cx.listener(move |view, event, window, cx| {
                                    view.choose_diff(index, true, event, window, cx)
                                }),
                            ))
                            .child(toast_button(
                                "Reject",
                                0x7d3b3b,
                                cx.listener(move |view, event, window, cx| {
                                    view.choose_diff(index, false, event, window, cx)
                                }),
                            )),
                    );
                }
                if matches!(card, Card::Permission { .. }) {
                    card_view = card_view.child(
                        div().flex().gap_2().children(
                            [
                                (PermissionChoice::AllowOnce, "Allow once", 0x39734a),
                                (PermissionChoice::AllowAlways, "Always", 0x39734a),
                                (PermissionChoice::Reject, "Reject", 0x7d3b3b),
                                (PermissionChoice::Inspect, "Inspect", 0x52627a),
                                (PermissionChoice::Cancel, "Cancel", 0x52627a),
                            ]
                            .into_iter()
                            .map(|(choice, label, color)| {
                                toast_button(
                                    label,
                                    color,
                                    cx.listener(move |view, event, window, cx| {
                                        view.choose_permission(index, choice, event, window, cx)
                                    }),
                                )
                            }),
                        ),
                    );
                }
                if matches!(card, Card::Question { answer: None, .. }) {
                    card_view = card_view.child(
                        div()
                            .flex()
                            .gap_2()
                            .child(toast_button(
                                "Yes",
                                0x39734a,
                                cx.listener(move |view, event, window, cx| {
                                    view.answer_question(index, "yes", event, window, cx)
                                }),
                            ))
                            .child(toast_button(
                                "No",
                                0x7d3b3b,
                                cx.listener(move |view, event, window, cx| {
                                    view.answer_question(index, "no", event, window, cx)
                                }),
                            )),
                    );
                }
                card_view
            }))
            .children(
                description
                    .transcript
                    .iter()
                    .skip(self.chat.cards.len())
                    .map(|(label, text)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .p_3()
                            .rounded_md()
                            // The event kind already selected this projection;
                            // presentation never reparses the label text to
                            // recover protocol semantics.
                            .bg(rgb(0x202630))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x8994a8))
                                    .child(label.clone()),
                            )
                            .child(text.clone())
                    }),
            );
        let sidebar = div()
            .w(px(260.))
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .flex_none()
            .border_l_1()
            .border_color(rgb(0x2c3340))
            .child(
                div()
                    .cursor_pointer()
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::click_agent))
                    .child(sidebar_card(
                        "AGENT",
                        description
                            .sidebar
                            .iter()
                            .find(|(title, _)| title == "AGENT")
                            .map(|(_, value)| value.as_str())
                            .unwrap_or(&self.agent),
                    )),
            )
            .children(
                description
                    .sidebar
                    .iter()
                    .filter(|(title, _)| title != "AGENT")
                    .map(|(title, value)| sidebar_card(title, value)),
            );
        let footer = div()
            .p_4()
            .border_t_1()
            .border_color(rgb(0x2c3340))
            .child(
                div()
                    .flex()
                    .gap_3()
                    .items_end()
                    .child(self.input.clone())
                    .child(
                        div()
                            .id("send-button")
                            .px_4()
                            .py_3()
                            .bg(rgb(0x85b8ff))
                            .hover(|style| style.bg(rgb(0xa8ccff)))
                            .focusable()
                            .text_color(rgb(0x10151e))
                            .rounded_lg()
                            .cursor_pointer()
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::click_send))
                            .child("Send"),
                    ),
            )
            .when(self.chat.active_assistant.is_some(), |footer| {
                footer.child(
                    div()
                        .id("cancel-button")
                        .px_3()
                        .py_2()
                        .bg(rgb(0x7d3b3b))
                        .hover(|style| style.bg(rgb(0xa34c4c)))
                        .focusable()
                        .rounded_lg()
                        .cursor_pointer()
                        .on_mouse_up(MouseButton::Left, cx.listener(Self::cancel_turn))
                        .child("Cancel"),
                )
            });
        let mut root = div()
            .size_full()
            .bg(rgb(0x11151b))
            .text_color(rgb(0xe8edf5))
            .flex()
            .flex_col()
            .min_h(px(0.))
            .relative()
            .key_context("TauView")
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::switch_agent));
        root = root.on_action(cx.listener(Self::cycle_model));
        root = root.on_action(cx.listener(Self::dismiss_picker));
        root = root.on_action(cx.listener(Self::picker_up));
        root = root.on_action(cx.listener(Self::picker_down));
        root = root.on_action(cx.listener(Self::pick_item));
        let command_active = self.picker.is_none() && command_mode(&self.input.read(cx).content());
        if let Some(kind) = self.picker {
            root = root.child(self.picker_overlay(cx, kind));
        } else if command_active {
            root = root.child(self.picker_overlay(cx, PickerKind::Command));
        }
        root = root.on_action(cx.listener(Self::toggle_sidebar));
        root = root.on_action(cx.listener(Self::toggle_follow));
        root.child(header)
            .when(self.sessions_open, |root| {
                root.child(self.session_panel(cx))
            })
            .children(runtime_banner_view)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.))
                    .child(transcript)
                    .when(self.sidebar_visible, |view| view.child(sidebar)),
            )
            .child(footer)
            .when(self.toast_visible, |root| {
                root.child(toast.absolute().top(px(0.)).left(px(0.)).right(px(0.)))
            })
    }
}

fn to_navigator_session(session: tau_client::SessionSummary) -> NavigatorSession {
    NavigatorSession {
        id: session.session_id.as_str().to_owned(),
        project_id: NavigatorProjectId::new(session.project_id.as_str()),
        title: session.title,
        updated_at: session.updated_at,
        archived: session.archived_at.is_some(),
    }
}

fn sidebar_card(title: &str, value: &str) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .rounded_md()
        .bg(rgb(0x1b1f27))
        .child(
            div()
                .text_xs()
                .text_color(rgb(0x7e8a9d))
                .child(title.to_string()),
        )
        .child(div().text_sm().child(value.to_string()))
}

fn toast_button(
    label: &str,
    color: u32,
    callback: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(rgb(color))
        .cursor_pointer()
        .on_mouse_up(MouseButton::Left, callback)
        .child(label.to_string())
}

#[cfg(test)]
mod view_model_tests {
    use super::*;

    #[test]
    fn description_exposes_empty_and_operational_states() {
        let state = ChatState::default();
        let view = describe_chat(&state, "Code", "model", "/tmp");
        assert!(!view.loading);
        assert!(view.transcript.is_empty());
        assert!(
            view.sidebar
                .iter()
                .any(|(key, value)| key == "LSP" && value == "Unavailable")
        );
    }

    #[test]
    fn description_keeps_lifecycle_and_error_events() {
        let mut state = ChatState::default();
        state.events.push(crate::chat::TypedEvent {
            event_id: "e".into(),
            session_id: "s".into(),
            sequence: 1,
            occurred_at: 0,
            request_id: None,
            turn_id: "t".into(),
            kind: EventKind::Failed {
                message: "boom".into(),
            },
            raw: tau_proto::prelude::TurnEvent::TurnFailed {
                turn_id: "t".into(),
                message: "boom".into(),
            },
        });
        let view = describe_chat(&state, "Plan", "model", "/tmp");
        assert!(
            view.transcript
                .iter()
                .any(|(label, text)| label == "error" && text == "boom")
        );
    }

    #[test]
    fn action_helpers_toggle_sidebar_and_follow_state() {
        assert!(next_sidebar_visibility(false));
        assert!(!next_sidebar_visibility(true));
        assert!(next_follow_state(false));
        assert!(!next_follow_state(true));
    }

    #[test]
    fn description_renders_typed_operational_fields_without_fabrication() {
        let mut state = ChatState::default();
        state.sidebar.plan = Some("release".into());
        state.sidebar.current_step = Some("render".into());
        state.sidebar.airtight = Some(false);
        state.sidebar.input_tokens = Some(10);
        state.sidebar.output_tokens = Some(4);
        state.sidebar.context_tokens = Some(20);
        state.sidebar.context_percent = Some(25);
        state.sidebar.lsp = Some("2 diagnostics".into());
        let view = describe_chat(&state, "Code", "model", "");
        assert!(
            view.sidebar
                .iter()
                .any(|(key, value)| key == "PLAN" && value.contains("not airtight"))
        );
        assert!(
            view.sidebar
                .iter()
                .any(|(key, value)| key == "TOKENS" && value.contains("25%"))
        );
        assert!(
            view.sidebar
                .iter()
                .any(|(key, value)| key == "LSP" && value == "2 diagnostics")
        );
        assert!(
            view.sidebar
                .iter()
                .any(|(key, value)| key == "DIRECTORY" && value == "Unavailable")
        );
    }
}
