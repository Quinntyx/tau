use gpui::{
    App, ClipboardItem, Context, Entity, Focusable, KeyBinding, MouseButton, MouseUpEvent, Render,
    ScrollHandle, StatefulInteractiveElement, Task, Window, deferred, div, prelude::*, px, rgb,
    rgba,
};
use std::path::PathBuf;
use tau_client::ProjectId;
use tau_client::TurnStreamEvent;
use tau_proto::prelude::{
    AuthState, ClientResponse, OPENAI_CODEX_PROVIDER, QuestionAnswer, TurnPermissionChoice,
};

use crate::backend::{Backend, DaemonAction, DaemonStatus};
use crate::chat::{Card, ChatAction, ChatState, ChatStatus, EventKind, PermissionChoice, Role};
use crate::composer::ComposerAction;
use crate::feed;
use crate::input::{TextInput, command_mode};
use crate::operations::OperationsModel;
use crate::picker::{
    AgentOption, ModelOption, PickerAction, command_action, command_suggestions, model_groups,
    next_agent, parse_command,
};
use crate::sessions::{
    Navigator, ProjectId as NavigatorProjectId, SessionSummary as NavigatorSession,
};
use crate::theme::Theme;

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
        ToggleSessions,
        OperationsStatus,
        OperationsGit,
        OperationsChanges,
        OperationsRefresh,
        OperationsStage,
        OperationsUnstage,
        OperationsRevert,
        OperationsKeep,
        OperationsAcknowledge,
        OperationsCreateBranch,
        OperationsSwitchBranch
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

fn feed_policy_request_id(item: &feed::FeedItem) -> Option<&str> {
    match item.category {
        feed::FeedCategory::Permission => item.actions.permission_id.as_deref(),
        feed::FeedCategory::Question => item.actions.question_id.as_deref(),
        feed::FeedCategory::Diff => item.actions.diff_request_id.as_deref(),
        _ => None,
    }
}

fn feed_item_is_pending(chat: &ChatState, item: &feed::FeedItem) -> bool {
    feed_policy_request_id(item)
        .and_then(|id| chat.policy_states.get(id))
        .is_none_or(|state| !matches!(state, crate::chat::PolicyState::Ack))
}

/// Render one projected feed body with a typography-first hierarchy.  The
/// reducer remains the source of truth; this seam deliberately accepts the
/// already projected label/text pair so protocol actions stay on `TauView`.
fn feed_body(label: &str, text: String) -> gpui::Div {
    let primary = matches!(label, "You" | "tau");
    div()
        .max_w(px(820.))
        .p_3()
        .rounded_lg()
        .when(primary, |element| element.text_base())
        .when(!primary, |element| element.text_sm())
        .child(text)
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
        loading: chat.loading || chat.is_streaming(),
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
        Card::Error {
            summary,
            details,
            expanded,
        } => (
            "error".into(),
            if *expanded {
                format!("{summary}\n{details}")
            } else {
                summary.clone()
            },
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
        KeyBinding::new("ctrl-shift-r", OperationsRefresh, None),
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
    /// Root paired with `selected_project_id`; project ids are not paths.
    selected_project_root: Option<PathBuf>,
    project_generation: u64,
    sessions: Navigator,
    session_query: Entity<TextInput>,
    sessions_open: bool,
    operations: OperationsModel,
    operations_loading: bool,
    operations_error: Option<String>,
    operations_tab: OperationsTab,
    account_sheet: bool,
    auth_status: AuthState,
    auth_loading: bool,
    auth_error: Option<String>,
    theme: Theme,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OperationsTab {
    Status,
    Git,
    Changes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
        let codex_models = [
            format!("{OPENAI_CODEX_PROVIDER}/gpt-5.6-sol"),
            format!("{OPENAI_CODEX_PROVIDER}/gpt-5.6"),
        ];
        for codex_model in &codex_models {
            if !models.iter().any(|candidate| candidate == codex_model) {
                models.push(codex_model.clone());
            }
        }
        for model in preferences
            .favorites
            .iter()
            .chain(preferences.recent_models.iter())
            .chain(preferences.selected_model.iter())
        {
            let model = if model.contains("gpt-5.4") {
                model.replace("gpt-5.4", "gpt-5.6-sol")
            } else {
                model.clone()
            };
            if !models.iter().any(|candidate| candidate == &model) {
                models.push(model);
            }
        }
        let selected_model = preferences.selected_model.as_ref().map(|m| {
            if m.contains("gpt-5.4") {
                m.replace("gpt-5.4", "gpt-5.6-sol")
            } else {
                m.clone()
            }
        });
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
        let initial_model = models
            .get(model_index)
            .cloned()
            .unwrap_or_else(|| default_model.clone());
        input.update(cx, |input, _| {
            input.set_model(initial_model);
            input.set_agent(agent.clone());
        });
        let mut view = Self {
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
            selected_project_root: None,
            project_generation: 0,
            sessions: Navigator::default(),
            session_query,
            sessions_open: false,
            operations: OperationsModel::new(),
            operations_loading: false,
            operations_error: None,
            operations_tab: OperationsTab::Status,
            account_sheet: false,
            auth_status: AuthState::SignedOut,
            auth_loading: false,
            auth_error: None,
            theme: Theme::default(),
        };
        view.refresh_auth(cx);
        view
    }

    /// Set the active project selected by the project navigator.  Turns are
    /// intentionally rejected until this explicit selection is supplied.
    pub fn select_project(
        &mut self,
        project_id: Option<String>,
        root: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let project_id = project_id.filter(|id| !id.trim().is_empty());
        if self.selected_project_id == project_id && self.selected_project_root == root {
            return;
        }
        self.selected_project_id = project_id;
        self.selected_project_root = root;
        self.project_generation = self.project_generation.wrapping_add(1);
        // Nothing from the previous repository may be displayed while the
        // daemon request for the new one is in flight.
        self.chat = ChatState::default();
        self.sessions = Navigator::default();
        self.operations = OperationsModel::new();
        self.operations_loading = false;
        self.operations_error = None;
        self.task = None;
        self.ack_tasks.clear();
        self.runtime = RuntimeState::NotNegotiated;
        if self.selected_project_id.is_some() {
            self.load_sessions(cx);
            self.refresh_operations(cx);
        }
        cx.notify();
    }

    fn operation_project(&self) -> Option<String> {
        self.selected_project_id.clone()
    }

    fn operation_project_or_error(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let project = self.operation_project();
        if project.is_none() {
            self.operations_loading = false;
            self.operations_error = Some("select a project before using repository actions".into());
            cx.notify();
        }
        project
    }

    fn project_is_current(&self, generation: u64, project_id: &str, wire_project_id: &str) -> bool {
        self.project_generation == generation
            && self.selected_project_id.as_deref() == Some(project_id)
            && self.operation_project().as_deref() == Some(wire_project_id)
    }

    fn refresh_operations(&mut self, cx: &mut Context<Self>) {
        let Some(project_root) = self.operation_project_or_error(cx) else {
            return;
        };
        let project_id = self.selected_project_id.clone().unwrap_or_default();
        let generation = self.project_generation;
        self.operations_loading = true;
        self.operations_error = None;
        cx.notify();
        let receiver = self.backend.operations_status(project_root.clone());
        cx.spawn(async move |view, cx| {
            let _ = match receiver.await {
                Ok(Ok(result)) => view.update(cx, |view, cx| {
                    if !view.project_is_current(generation, &project_id, &project_root) {
                        return;
                    }
                    view.operations.apply_status(result.branch, result.files);
                    if !view.project_is_current(generation, &project_id, &project_root) {
                        return;
                    }
                    view.operations_loading = false;
                    cx.notify();
                }),
                Ok(Err(error)) => view.update(cx, |view, cx| {
                    if !view.project_is_current(generation, &project_id, &project_root) {
                        return;
                    }
                    view.operations_loading = false;
                    view.operations_error = Some(error.to_string());
                    cx.notify();
                }),
                Err(error) => view.update(cx, |view, cx| {
                    if !view.project_is_current(generation, &project_id, &project_root) {
                        return;
                    }
                    view.operations_loading = false;
                    view.operations_error = Some(error.to_string());
                    cx.notify();
                }),
            };
        })
        .detach();
        self.refresh_operation_branches(cx);
    }

    fn refresh_operation_branches(&mut self, cx: &mut Context<Self>) {
        let Some(project_root) = self.operation_project_or_error(cx) else {
            return;
        };
        let project_id = self.selected_project_id.clone().unwrap_or_default();
        let generation = self.project_generation;
        let receiver = self.backend.operations_branches(project_root.clone());
        cx.spawn(async move |view, cx| {
            if let Ok(Ok(result)) = receiver.await {
                let _ = view.update(cx, |view, cx| {
                    if !view.project_is_current(generation, &project_id, &project_root) {
                        return;
                    }
                    view.operations.branch = result.current;
                    view.operations.apply_branches(result.branches);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn set_operations_tab(&mut self, tab: OperationsTab, cx: &mut Context<Self>) {
        self.operations_tab = tab;
        cx.notify();
    }

    fn open_operation_file(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(project_root) = self.operation_project_or_error(cx) else {
            return;
        };
        let project_id = self.selected_project_id.clone().unwrap_or_default();
        let generation = self.project_generation;
        self.operations_loading = true;
        self.operations_error = None;
        cx.notify();
        let receiver = self.backend.operations_file(project_root.clone(), path);
        cx.spawn(async move |view, cx| match receiver.await {
            Ok(Ok(file)) => {
                let _ = view.update(cx, |view, cx| {
                    if !view.project_is_current(generation, &project_id, &project_root) {
                        return;
                    }
                    let revision = crate::operations::content_revision(&file.content);
                    view.operations.invalidate_revision(&file.path, &revision);
                    view.operations.select_file(file);
                    view.operations_loading = false;
                    cx.notify();
                });
            }
            Ok(Err(error)) => {
                let _ = view.update(cx, |view, cx| {
                    if !view.project_is_current(generation, &project_id, &project_root) {
                        return;
                    }
                    view.operations_loading = false;
                    view.operations_error = Some(error.to_string());
                    cx.notify();
                });
            }
            Err(error) => {
                let _ = view.update(cx, |view, cx| {
                    if !view.project_is_current(generation, &project_id, &project_root) {
                        return;
                    }
                    view.operations_loading = false;
                    view.operations_error = Some(error.to_string());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn operation_path(&self) -> Option<String> {
        self.operations
            .selected
            .as_ref()
            .map(|file| file.path.clone())
    }

    fn operation_mutation(&mut self, path: String, kind: &'static str, cx: &mut Context<Self>) {
        let Some(project_root) = self.operation_project_or_error(cx) else {
            return;
        };
        let project_id = self.selected_project_id.clone().unwrap_or_default();
        let generation = self.project_generation;
        self.operations_loading = true;
        self.operations_error = None;
        cx.notify();
        let receiver = match kind {
            "stage" => self.backend.operations_stage(project_root.clone(), path),
            "unstage" => self.backend.operations_unstage(project_root.clone(), path),
            "revert" => self
                .backend
                .operations_revert(project_root.clone(), path, true),
            _ => return,
        };
        cx.spawn(async move |view, cx| match receiver.await {
            Ok(Ok(_)) => {
                let _ = view.update(cx, |view, cx| {
                    if !view.project_is_current(generation, &project_id, &project_root) {
                        return;
                    }
                    view.operations_loading = false;
                    view.operations.acknowledge(true);
                    view.refresh_operations(cx);
                    cx.notify();
                });
            }
            Ok(Err(error)) => {
                let _ = view.update(cx, |view, cx| {
                    if !view.project_is_current(generation, &project_id, &project_root) {
                        return;
                    }
                    view.operations_loading = false;
                    view.operations_error = Some(error.to_string());
                    cx.notify();
                });
            }
            Err(error) => {
                let _ = view.update(cx, |view, cx| {
                    if !view.project_is_current(generation, &project_id, &project_root) {
                        return;
                    }
                    view.operations_loading = false;
                    view.operations_error = Some(error.to_string());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn keep_operation(&mut self, cx: &mut Context<Self>) {
        if let Some(file) = self.operations.selected.as_ref() {
            let revision = crate::operations::content_revision(&file.content);
            self.operations.toggle_keep(file.path.clone(), revision);
            cx.notify();
        }
    }

    fn acknowledge_operation(&mut self, cx: &mut Context<Self>) {
        let Some(project_root) = self.operation_project_or_error(cx) else {
            return;
        };
        self.operations_loading = true;
        self.operations_error = None;
        cx.notify();
        let project_id = self.selected_project_id.clone().unwrap_or_default();
        let generation = self.project_generation;
        let receiver = self
            .backend
            .operations_ack(project_root.clone(), "operations".into(), true);
        cx.spawn(async move |view, cx| match receiver.await {
            Ok(Ok(result)) => {
                let _ = view.update(cx, |view, cx| {
                    if !view.project_is_current(generation, &project_id, &project_root) {
                        return;
                    }
                    view.operations_loading = false;
                    view.operations.acknowledge(result.acknowledged);
                    cx.notify();
                });
            }
            Ok(Err(error)) => {
                let _ = view.update(cx, |view, cx| {
                    if !view.project_is_current(generation, &project_id, &project_root) {
                        return;
                    }
                    view.operations_loading = false;
                    view.operations_error = Some(error);
                    cx.notify();
                });
            }
            Err(error) => {
                let _ = view.update(cx, |view, cx| {
                    if !view.project_is_current(generation, &project_id, &project_root) {
                        return;
                    }
                    view.operations_loading = false;
                    view.operations_error = Some(error.to_string());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn create_operation_branch(&mut self, cx: &mut Context<Self>) {
        let Some(project_root) = self.operation_project_or_error(cx) else {
            return;
        };
        let project_id = self.selected_project_id.clone().unwrap_or_default();
        let generation = self.project_generation;
        self.operations_loading = true;
        self.operations_error = None;
        cx.notify();
        let receiver = self
            .backend
            .operations_create_branch(project_root.clone(), "tau/gui-review".into());
        cx.spawn(async move |view, cx| {
            let result = receiver.await;
            let _ = view.update(cx, |view, cx| {
                if !view.project_is_current(generation, &project_id, &project_root) {
                    return;
                }
                view.operations_loading = false;
                if let Ok(Err(error)) = result {
                    view.operations_error = Some(error.to_string());
                }
                view.refresh_operation_branches(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn switch_operation_branch(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(project_root) = self.operation_project_or_error(cx) else {
            return;
        };
        let project_id = self.selected_project_id.clone().unwrap_or_default();
        let generation = self.project_generation;
        self.operations_loading = true;
        self.operations_error = None;
        cx.notify();
        let receiver = self
            .backend
            .operations_switch_branch(project_root.clone(), name);
        cx.spawn(async move |view, cx| match receiver.await {
            Ok(Ok(_)) => {
                let _ = view.update(cx, |view, cx| {
                    if !view.project_is_current(generation, &project_id, &project_root) {
                        return;
                    }
                    view.operations_loading = false;
                    view.refresh_operations(cx);
                    cx.notify();
                });
            }
            Ok(Err(error)) => {
                let _ = view.update(cx, |view, cx| {
                    if !view.project_is_current(generation, &project_id, &project_root) {
                        return;
                    }
                    view.operations_loading = false;
                    view.operations_error = Some(error.to_string());
                    cx.notify();
                });
            }
            Err(error) => {
                let _ = view.update(cx, |view, cx| {
                    if !view.project_is_current(generation, &project_id, &project_root) {
                        return;
                    }
                    view.operations_loading = false;
                    view.operations_error = Some(error.to_string());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn operations_refresh_action(
        &mut self,
        _: &OperationsRefresh,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.refresh_operations(cx);
    }

    fn operations_status_action(
        &mut self,
        _: &OperationsStatus,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_operations_tab(OperationsTab::Status, cx);
    }

    fn operations_git_action(&mut self, _: &OperationsGit, _: &mut Window, cx: &mut Context<Self>) {
        self.set_operations_tab(OperationsTab::Git, cx);
    }

    fn operations_changes_action(
        &mut self,
        _: &OperationsChanges,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_operations_tab(OperationsTab::Changes, cx);
    }

    fn operations_stage_action(
        &mut self,
        _: &OperationsStage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(path) = self.operation_path() {
            self.operation_mutation(path, "stage", cx);
        }
    }

    fn operations_unstage_action(
        &mut self,
        _: &OperationsUnstage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(path) = self.operation_path() {
            self.operation_mutation(path, "unstage", cx);
        }
    }

    fn operations_revert_action(
        &mut self,
        _: &OperationsRevert,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(path) = self.operation_path() {
            self.operation_mutation(path, "revert", cx);
        }
    }

    fn operations_keep_action(
        &mut self,
        _: &OperationsKeep,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.keep_operation(cx);
    }

    fn operations_acknowledge_action(
        &mut self,
        _: &OperationsAcknowledge,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.acknowledge_operation(cx);
    }

    fn operations_create_action(
        &mut self,
        _: &OperationsCreateBranch,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.create_operation_branch(cx);
    }

    fn operations_switch_action(
        &mut self,
        _: &OperationsSwitchBranch,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(branch) = self
            .operations
            .branches
            .iter()
            .find(|branch| !branch.current)
        {
            self.switch_operation_branch(branch.name.clone(), cx);
        }
    }

    fn operations_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let status = if self.operations_loading {
            "Loading…"
        } else if let Some(error) = &self.operations_error {
            error.as_str()
        } else {
            "Ready"
        };
        let branch = if self.operations.branch.is_empty() {
            "(unknown)"
        } else {
            &self.operations.branch
        };
        let acknowledgement = self
            .operations
            .acknowledgement
            .map(|value| {
                if value {
                    "acknowledged"
                } else {
                    "not acknowledged"
                }
            })
            .unwrap_or("—");
        let active_tab = match self.operations_tab {
            OperationsTab::Status => "Status",
            OperationsTab::Git => "Git",
            OperationsTab::Changes => "Changes",
        };
        let mut panel = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(self.theme.text)
                    .child("Repository"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(self.theme.tertiary_text)
                    .child(format!("Status · {status}")),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(self.theme.secondary_text)
                    .child(format!("Branch: {branch}")),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(self.theme.tertiary_text)
                    .child(format!("Ack · {acknowledgement}")),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(self.theme.tertiary_text)
                    .child(format!("Tab · {active_tab}")),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(composer_pill(
                        "Status",
                        "operations-status",
                        self.theme,
                        cx.listener(|view, _, _, cx| {
                            view.set_operations_tab(OperationsTab::Status, cx)
                        }),
                    ))
                    .child(composer_pill(
                        "Git",
                        "operations-git",
                        self.theme,
                        cx.listener(|view, _, _, cx| {
                            view.set_operations_tab(OperationsTab::Git, cx)
                        }),
                    ))
                    .child(composer_pill(
                        "Changes",
                        "operations-changes",
                        self.theme,
                        cx.listener(|view, _, _, cx| {
                            view.set_operations_tab(OperationsTab::Changes, cx)
                        }),
                    ))
                    .child(composer_pill(
                        "Refresh",
                        "operations-refresh",
                        self.theme,
                        cx.listener(|view, _, _, cx| view.refresh_operations(cx)),
                    )),
            );
        panel = panel.children(self.operations.files.iter().map(|file| {
            let path = file.path.clone();
            let callback_path = path.clone();
            div()
                .flex()
                .justify_between()
                .child(composer_pill(
                    path,
                    "operation-file",
                    self.theme,
                    cx.listener(move |view, _, _, cx| {
                        view.open_operation_file(callback_path.clone(), cx)
                    }),
                ))
                .child(if file.staged {
                    "staged"
                } else if file.untracked {
                    "untracked"
                } else {
                    "modified"
                })
        }));
        if let Some(file) = &self.operations.selected {
            let stage_path = file.path.clone();
            let unstage_path = file.path.clone();
            let revert_path = file.path.clone();
            let revision = crate::operations::content_revision(&file.content);
            let keep_label = if self.operations.is_kept(&file.path, &revision) {
                "Unkeep"
            } else {
                "Keep"
            };
            let mut details = div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_color(self.theme.text).child(format!(
                    "{}\n\nCONTENT\n{}\nDIFF\n{}",
                    file.path, file.content, file.diff
                )))
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(composer_pill(
                            "Stage",
                            "op-stage",
                            self.theme,
                            cx.listener(move |view, _, _, cx| {
                                view.operation_mutation(stage_path.clone(), "stage", cx)
                            }),
                        ))
                        .child(composer_pill(
                            "Unstage",
                            "op-unstage",
                            self.theme,
                            cx.listener(move |view, _, _, cx| {
                                view.operation_mutation(unstage_path.clone(), "unstage", cx)
                            }),
                        ))
                        .child(mac_button(
                            "Revert (confirm)",
                            "op-revert",
                            false,
                            self.theme,
                            cx.listener(move |view, _, _, cx| {
                                view.operation_mutation(revert_path.clone(), "revert", cx)
                            }),
                        ))
                        .child(composer_pill(
                            keep_label,
                            "op-keep",
                            self.theme,
                            cx.listener(|view, _, _, cx| view.keep_operation(cx)),
                        )),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(self.theme.secondary_text)
                        .child("Branches"),
                );
            details = details.children(self.operations.branches.iter().map(|branch| {
                let name = branch.name.clone();
                let callback_name = name.clone();
                composer_pill(
                    name,
                    "op-branch",
                    self.theme,
                    cx.listener(move |view, _, _, cx| {
                        view.switch_operation_branch(callback_name.clone(), cx)
                    }),
                )
            }));
            details = details.child(composer_pill(
                "Create tau/gui-review",
                "op-create-branch",
                self.theme,
                cx.listener(|view, _, _, cx| view.create_operation_branch(cx)),
            ));
            details = details.child(composer_pill(
                "Acknowledge",
                "op-acknowledge",
                self.theme,
                cx.listener(|view, _, _, cx| view.acknowledge_operation(cx)),
            ));
            panel = panel.child(details);
        }
        panel
    }

    fn account_sheet(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let content = div()
            .w(px(520.))
            .max_w_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_6()
            .rounded_xl()
            .border_1()
            .border_color(self.theme.separator)
            .bg(self.theme.elevated)
            .text_color(self.theme.text)
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_xl().child("ChatGPT account"))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(self.theme.secondary_text)
                                    .child("Use your Plus or Pro subscription with Codex."),
                            ),
                    )
                    .child(mac_button(
                        "Close",
                        "account-close",
                        false,
                        self.theme,
                        cx.listener(Self::close_account),
                    )),
            )
            .when_some(self.auth_error.clone(), |view, error| {
                view.child(
                    div()
                        .p_3()
                        .rounded_md()
                        .bg(self.theme.error_surface)
                        .text_color(self.theme.error_text)
                        .child(error),
                )
            });
        let content = match &self.auth_status {
            AuthState::SignedOut => content
                .child(
                    div()
                        .text_sm()
                        .text_color(self.theme.secondary_text)
                        .child("Tau will open a secure browser authorization page. Your password is never visible to Tau."),
                )
                .child(mac_button(
                    if self.auth_loading { "Connecting..." } else { "Connect ChatGPT" },
                    "account-connect",
                    true,
                    self.theme,
                    cx.listener(Self::begin_auth),
                )),
            AuthState::Pending { authorize_url } => content
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().text_sm().child("Authorization is waiting in your browser."))
                        .child(
                            div()
                                .p_3()
                                .rounded_md()
                                .bg(self.theme.canvas)
                                .text_xs()
                                .text_color(self.theme.accent)
                                .child(authorize_url.clone()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(mac_button(
                            "Open Browser",
                            "account-open-browser",
                            true,
                            self.theme,
                            cx.listener(Self::open_auth_url),
                        ))
                        .child(mac_button(
                            "Copy Link",
                            "account-copy-link",
                            false,
                            self.theme,
                            cx.listener(Self::copy_auth_url),
                        ))
                        .child(mac_button(
                            "Cancel",
                            "account-cancel",
                            false,
                            self.theme,
                            cx.listener(Self::cancel_auth),
                        )),
                ),
            AuthState::SignedIn { email, account_id } => content
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_3()
                        .rounded_md()
                        .bg(self.theme.success_surface)
                        .text_color(self.theme.success_text)
                        .child("Connected")
                        .children(email.iter().cloned().map(|value| div().text_sm().child(value)))
                        .children(account_id.iter().cloned().map(|value| {
                            div()
                                .text_xs()
                                .text_color(self.theme.secondary_text)
                                .child(value)
                        })),
                )
                .child(testable_button(
                    "Sign Out",
                    0x8b3038,
                    "account-logout",
                    cx.listener(Self::logout_auth),
                )),
            AuthState::Failed { message } => content
                .child(
                    div()
                        .p_3()
                        .rounded_md()
                        .bg(self.theme.error_surface)
                        .text_color(self.theme.error_text)
                        .child(message.clone()),
                )
                .child(mac_button(
                    "Try Again",
                    "account-retry",
                    true,
                    self.theme,
                    cx.listener(Self::begin_auth),
                )),
        };
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .p_6()
            .bg(rgba(0x00000099))
            .child(content)
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
        let Some(project_root) = self
            .selected_project_root
            .as_ref()
            .map(|root| root.to_string_lossy().into_owned())
        else {
            self.runtime =
                RuntimeState::Failed("select a project root before creating a chat".into());
            cx.notify();
            return;
        };
        let project_id = project.as_str().to_owned();
        let generation = self.project_generation;
        self.runtime = RuntimeState::Negotiating;
        cx.notify();
        self.ack_tasks.push(cx.spawn(async move |this, cx| {
            let result = match backend.session_create(project, project_root.clone()).await {
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
                if view.project_is_current(generation, &project_id, &project_root) {
                    if let Some(summary) = result {
                        view.chat.session_id = Some(summary.session_id.as_str().to_owned());
                        view.chat.cards.clear();
                        view.chat.events.clear();
                        view.chat.status = ChatStatus::Ready;
                        view.runtime = RuntimeState::Ready;
                        view.load_sessions(cx);
                        cx.notify();
                    } else {
                        view.runtime = RuntimeState::Failed("could not create a new chat".into());
                        cx.notify();
                    }
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
        let selected_id = project.as_str().to_owned();
        let generation = self.project_generation;
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
                if view.project_generation != generation
                    || view.selected_project_id.as_deref() != Some(selected_id.as_str())
                {
                    return;
                }
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
        let selected_project_id = project.as_str().to_owned();
        let generation = self.project_generation;
        let reference = tau_client::SessionRef {
            project_id: project.clone(),
            session_id: tau_client::SessionId::new(session_id.clone()),
        };
        self.sessions.restore_selection(Some(&session_id));
        self.sessions_open = false;
        self.runtime = RuntimeState::Negotiating;
        self.chat.cards.clear();
        self.chat.events.clear();
        cx.notify();
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
                if view.project_generation != generation
                    || view.selected_project_id.as_deref() != Some(selected_project_id.as_str())
                {
                    return;
                }
                if let Some(history) = history {
                    view.chat.cards.clear();
                    view.chat.events.clear();
                    view.chat.session_id = Some(session_id);
                    view.runtime = RuntimeState::Ready;
                    for event in history.entries {
                        view.chat.reduce(ChatAction::SessionEvent(event));
                    }
                } else {
                    view.runtime = RuntimeState::Failed("could not load session history".into());
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
        if self.selected_project_id.as_deref() != Some(project_id.as_str()) {
            return;
        }
        let generation = self.project_generation;
        let update_project_id = project_id.clone();
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
                if view.project_generation != generation
                    || view.selected_project_id.as_deref() != Some(update_project_id.as_str())
                {
                    return;
                }
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
        if self.selected_project_id.as_deref() != Some(project_id.as_str()) {
            return;
        }
        let generation = self.project_generation;
        let update_project_id = project_id.clone();
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
                if view.project_generation != generation
                    || view.selected_project_id.as_deref() != Some(update_project_id.as_str())
                {
                    return;
                }
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
                        cx.stop_propagation();
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
                                );
                                cx.stop_propagation();
                            }),
                        ))
                        .child(lifecycle_button),
                );
                row
            }))
    }

    fn switch_agent(&mut self, _: &SwitchAgent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(next) = next_agent(&self.agents, Some(&self.agent), false) {
            self.select_agent(next, cx);
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
                PickerKind::Model => self.select_model(item, cx),
                PickerKind::Agent => self.select_agent(item, cx),
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
        self.input.update(cx, |input, _| {
            input.dispatch(ComposerAction::ChooseCompletion(value));
        });
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

    fn toggle_error(
        &mut self,
        index: usize,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(Card::Error { expanded, .. }) = self.chat.cards.get_mut(index) {
            *expanded = !*expanded;
            cx.notify();
        }
    }

    fn retry_last_prompt(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.chat.raw_input.clone() else {
            return;
        };
        self.input.update(cx, |input, _| input.set_content(prompt));
        self.send(window, cx);
    }

    fn card_index_for_request(
        &self,
        request_id: &str,
        category: feed::FeedCategory,
    ) -> Option<usize> {
        self.chat
            .cards
            .iter()
            .position(|card| match (category, card) {
                (feed::FeedCategory::Permission, Card::Permission { request_id: id, .. })
                | (feed::FeedCategory::Diff, Card::Diff { request_id: id, .. }) => id == request_id,
                (
                    feed::FeedCategory::Question,
                    Card::Question {
                        question_id: id, ..
                    },
                ) => id == request_id,
                _ => false,
            })
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
        self.input.update(cx, |input, _| {
            input.dispatch(ComposerAction::Cancel);
        });
        self.backend.cancel();
        self.chat.active_assistant = None;
        self.chat.status = ChatStatus::Ready;
        self.task = None;
        self.input.update(cx, |input, _| input.set_disabled(false));
        cx.notify();
    }

    fn retry_runtime(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.refresh_auth(cx);
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

    fn selected_model(&self) -> &str {
        self.models
            .get(self.model_index)
            .map(String::as_str)
            .unwrap_or_default()
    }

    fn codex_selected(&self) -> bool {
        self.selected_model()
            .starts_with(&format!("{OPENAI_CODEX_PROVIDER}/"))
    }

    fn refresh_auth(&mut self, cx: &mut Context<Self>) {
        self.auth_loading = true;
        self.auth_error = None;
        let request = self.backend.auth_status();
        let task = cx.spawn(async move |view, cx| {
            let result = request.await;
            let _ = view.update(cx, |view, cx| {
                view.auth_loading = false;
                match result {
                    Ok(Ok(result)) => {
                        let signed_in = matches!(result.status, AuthState::SignedIn { .. });
                        view.auth_status = result.status;
                        if signed_in {
                            view.account_sheet = false;
                            if matches!(&view.runtime, RuntimeState::Failed(msg) if msg.contains("ChatGPT")) {
                                view.runtime = RuntimeState::NotNegotiated;
                                view.backend.daemon_action(DaemonAction::Retry);
                            }
                        }
                    }
                    Ok(Err(error)) => view.auth_error = Some(error.to_string()),
                    Err(error) => view.auth_error = Some(error.to_string()),
                }
                cx.notify();
            });
        });
        self.ack_tasks.push(task);
    }

    fn open_account(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.account_sheet = true;
        self.refresh_auth(cx);
        cx.notify();
    }

    fn close_account(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.account_sheet = false;
        cx.notify();
    }

    fn begin_auth(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.auth_loading = true;
        self.auth_error = None;
        let request = self.backend.auth_begin();
        let backend = self.backend.clone();
        let task = cx.spawn(async move |view, cx| match request.await {
            Ok(Ok(result)) => {
                let _ = view.update(cx, |view, cx| {
                    view.auth_status = result.status;
                    view.auth_loading = false;
                    cx.notify();
                });
                let wait = backend.auth_wait();
                let completed = wait.await;
                let _ = view.update(cx, |view, cx| {
                    match completed {
                        Ok(Ok(result)) => {
                            let signed_in = matches!(result.status, AuthState::SignedIn { .. });
                            view.auth_status = result.status;
                            view.auth_error = None;
                            if signed_in {
                                view.account_sheet = false;
                                view.runtime = RuntimeState::NotNegotiated;
                                view.backend.daemon_action(DaemonAction::Retry);
                            }
                        }
                        Ok(Err(error)) => view.auth_error = Some(error.to_string()),
                        Err(error) => view.auth_error = Some(error.to_string()),
                    }
                    view.auth_loading = false;
                    cx.notify();
                });
            }
            Ok(Err(error)) => {
                let _ = view.update(cx, |view, cx| {
                    view.auth_loading = false;
                    view.auth_error = Some(error.to_string());
                    cx.notify();
                });
            }
            Err(error) => {
                let _ = view.update(cx, |view, cx| {
                    view.auth_loading = false;
                    view.auth_error = Some(error.to_string());
                    cx.notify();
                });
            }
        });
        self.ack_tasks.push(task);
        cx.notify();
    }

    fn open_auth_url(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let AuthState::Pending { authorize_url } = &self.auth_status {
            cx.open_url(authorize_url);
        }
    }

    fn copy_auth_url(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let AuthState::Pending { authorize_url } = &self.auth_status {
            cx.write_to_clipboard(ClipboardItem::new_string(authorize_url.clone()));
        }
    }

    fn cancel_auth(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let request = self.backend.auth_cancel();
        self.auth_loading = true;
        let task = cx.spawn(async move |view, cx| {
            let result = request.await;
            let _ = view.update(cx, |view, cx| {
                view.auth_loading = false;
                match result {
                    Ok(Ok(result)) => view.auth_status = result.status,
                    Ok(Err(error)) => view.auth_error = Some(error.to_string()),
                    Err(error) => view.auth_error = Some(error.to_string()),
                }
                cx.notify();
            });
        });
        self.ack_tasks.push(task);
        cx.notify();
    }

    fn logout_auth(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let request = self.backend.auth_logout();
        self.auth_loading = true;
        let task = cx.spawn(async move |view, cx| {
            let result = request.await;
            let _ = view.update(cx, |view, cx| {
                view.auth_loading = false;
                match result {
                    Ok(Ok(result)) => view.auth_status = result.status,
                    Ok(Err(error)) => view.auth_error = Some(error.to_string()),
                    Err(error) => view.auth_error = Some(error.to_string()),
                }
                cx.notify();
            });
        });
        self.ack_tasks.push(task);
        cx.notify();
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chat.is_streaming() {
            return;
        }
        let prompt = self.input.read(cx).content();
        if prompt.trim().is_empty() {
            return;
        }
        if self.selected_project_id.is_none() {
            self.runtime = RuntimeState::Failed("select a project before sending".into());
            cx.notify();
            return;
        }
        if matches!(
            self.backend.daemon_status(),
            DaemonStatus::Incompatible | DaemonStatus::Failed
        ) {
            self.runtime = RuntimeState::Failed(
                "Tau cannot send until the daemon connection is repaired. Restart an external daemon, then choose Retry.".into(),
            );
            cx.notify();
            return;
        }
        if self.codex_selected() && !matches!(self.auth_status, AuthState::SignedIn { .. }) {
            self.account_sheet = true;
            self.runtime = RuntimeState::Failed(
                "Connect a ChatGPT Plus or Pro account before using the Codex model.".into(),
            );
            cx.notify();
            return;
        }
        if let Some(command) = parse_command(&prompt) {
            if let Some(action) = command_action(command) {
                match action {
                    PickerAction::OpenModels => self.open_picker(PickerKind::Model, window, cx),
                    PickerAction::OpenAgents => self.open_picker(PickerKind::Agent, window, cx),
                    PickerAction::SelectModel(model) => self.select_model(model, cx),
                    PickerAction::SelectAgent(agent) => self.select_agent(agent, cx),
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
            input.dispatch(ComposerAction::Send);
            input.record_submission(prompt.clone());
            input.reset();
            input.set_disabled(true);
        });
        self.chat.reduce(ChatAction::Submit(prompt.clone()));
        self.runtime = RuntimeState::Negotiating;
        cx.notify();
        window.focus(&self.input.focus_handle(cx));

        let selected_model = self.models.get(self.model_index).cloned();
        let generation = self.project_generation;
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
                    if view.project_generation != generation {
                        return;
                    }
                    view.apply_event(event, cx);
                });
                if done {
                    break;
                }
            }
        }));
    }

    fn select_model(&mut self, model: String, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, _| input.set_model(model.clone()));
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

    fn select_agent(&mut self, agent: String, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, _| input.set_agent(agent.clone()));
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
                .rounded_md()
                .bg(self.theme.surface)
                .text_color(self.theme.text)
                .child(format!("Command: {}", query))
        } else {
            div().mt_2().child(self.picker_input.clone())
        };
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(120.))
            .bg(rgba(0x00000099))
            .child(
                div()
                    .key_context("Picker")
                    .w(px(480.))
                    .max_h(px(520.))
                    .p_4()
                    .rounded_xl()
                    .bg(self.theme.elevated)
                    .border_1()
                    .border_color(self.theme.separator)
                    .shadow_lg()
                    .child(
                        div()
                            .text_base()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(self.theme.text)
                            .child(title),
                    )
                    .child(search)
                    .children(rows.into_iter().enumerate().map(
                        |(display_index, (row, item_index))| {
                            let selected = item_index == Some(self.picker_index);
                            let is_header = item_index.is_none();
                            let favorite_model = (kind == PickerKind::Model)
                                .then(|| item_index.and_then(|index| items.get(index).cloned()))
                                .flatten();
                            let mut row_view = div()
                                .id(("picker-row", display_index))
                                .mt_1()
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .text_color(if is_header {
                                    self.theme.secondary_text
                                } else {
                                    self.theme.text
                                })
                                .text_xs()
                                .when(is_header, |d| {
                                    d.font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(self.theme.tertiary_text)
                                })
                                .when(selected, |d| {
                                    d.bg(self.theme.accent).text_color(rgb(0xffffff))
                                })
                                .when(!is_header && !selected, |d| {
                                    d.hover(|style| style.bg(self.theme.surface))
                                })
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
                                                cx.stop_propagation();
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
                            .text_color(self.theme.tertiary_text)
                            .child("↑↓ navigate  Enter select  Esc close"),
                    ),
            )
    }

    fn apply_event(&mut self, event: Result<TurnStreamEvent, String>, cx: &mut Context<Self>) {
        match event {
            Ok(TurnStreamEvent::Event(event)) => {
                self.chat.reduce(ChatAction::SessionEvent(event));
                self.runtime = match &self.chat.status {
                    ChatStatus::Failed(error) => RuntimeState::Failed(error.clone()),
                    _ => RuntimeState::Ready,
                };
                if let Some(session_id) = self.chat.session_id.clone() {
                    if let Some(project_id) = self.selected_project_id.clone() {
                        self.input.update(cx, |input, _| {
                            input.set_ids(session_id, project_id);
                        });
                    }
                }
                cx.notify();
            }
            Ok(TurnStreamEvent::Complete(_)) => {
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = Theme::for_appearance(window.appearance());
        let current_model = self
            .models
            .get(self.model_index)
            .map(String::as_str)
            .unwrap_or(self.backend.model());
        let send_blocked = self.chat.is_streaming()
            || matches!(
                self.backend.daemon_status(),
                DaemonStatus::Incompatible | DaemonStatus::Failed
            )
            || (current_model.starts_with(&format!("{OPENAI_CODEX_PROVIDER}/"))
                && !matches!(self.auth_status, AuthState::SignedIn { .. }));
        let description = describe_chat(&self.chat, &self.agent, current_model, self.backend.cwd());
        let typed_feed = feed::FeedProjection::from_events(&self.chat.events);
        if self.follow_output {
            self.transcript_scroll.scroll_to_bottom();
        }
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(52.))
            .px_4()
            .border_b_1()
            .border_color(self.theme.separator)
            .bg(self.theme.toolbar)
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Tau"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(mac_button(
                        "New Chat",
                        "new-chat-button",
                        true,
                        self.theme,
                        cx.listener(Self::new_chat),
                    ))
                    .child(mac_button(
                        if self.sessions_open {
                            "Hide sessions"
                        } else {
                            "Sessions"
                        },
                        "sessions-button",
                        false,
                        self.theme,
                        cx.listener(Self::toggle_sessions),
                    ))
                    .child(mac_button(
                        if self.sidebar_visible {
                            "Hide Inspector"
                        } else {
                            "Show Inspector"
                        },
                        "sidebar-button",
                        false,
                        self.theme,
                        cx.listener(|view, _, _, cx| {
                            view.sidebar_visible = next_sidebar_visibility(view.sidebar_visible);
                            cx.notify();
                        }),
                    ))
                    .child(mac_button(
                        if self.follow_output {
                            "Following"
                        } else {
                            "Follow output"
                        },
                        "follow-button",
                        false,
                        self.theme,
                        cx.listener(|view, _, _, cx| {
                            view.follow_output = next_follow_state(view.follow_output);
                            if view.follow_output {
                                view.transcript_scroll.scroll_to_bottom();
                            }
                            cx.notify();
                        }),
                    ))
                    .child(mac_button(
                        "Settings",
                        "settings-button",
                        false,
                        self.theme,
                        cx.listener(Self::open_account),
                    ))
                    .child(
                        div()
                            .text_sm()
                            .text_color(self.theme.secondary_text)
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
                .bg(if failed {
                    self.theme.error_surface
                } else {
                    self.theme.selection
                })
                .text_color(if failed {
                    self.theme.error_text
                } else {
                    self.theme.text
                })
                .child(div().flex_1().child(message.to_string()));
            if failed {
                banner = banner.child(mac_button(
                    "Retry",
                    "banner-retry",
                    true,
                    self.theme,
                    cx.listener(Self::retry_runtime),
                ));
                if self.backend.owns_daemon() {
                    banner = banner.child(mac_button(
                        "Restart Daemon",
                        "banner-restart",
                        false,
                        self.theme,
                        cx.listener(Self::restart_runtime),
                    ));
                }
            }
            if self.backend.owns_daemon() {
                banner = banner.child(mac_button(
                    "Disown",
                    "banner-disown",
                    false,
                    self.theme,
                    cx.listener(Self::disown_daemon),
                ));
            }
            banner.child(mac_button(
                "Quit",
                "banner-quit",
                false,
                self.theme,
                cx.listener(Self::quit_gui),
            ))
        });
        let toast =
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .p_3()
                .bg(self.theme.elevated)
                .border_1()
                .border_color(self.theme.separator)
                .text_color(self.theme.text)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child("Tau started the daemon automatically for this session.")
                        .child(div().text_xs().text_color(self.theme.secondary_text).child(
                            "The daemon remains owned by this window unless you disown it.",
                        )),
                )
                .child(
                    div()
                        .flex()
                        .id("startup-actions")
                        .max_h(px(420.))
                        .overflow_y_scroll()
                        .gap_2()
                        .child(mac_button(
                            "Okay",
                            "toast-okay",
                            true,
                            self.theme,
                            cx.listener(Self::hide_toast),
                        ))
                        .child(mac_button(
                            "Don't show again",
                            "toast-never",
                            false,
                            self.theme,
                            cx.listener(Self::never_show_toast),
                        ))
                        .child(mac_button(
                            "Disown",
                            "toast-disown",
                            false,
                            self.theme,
                            cx.listener(Self::disown_daemon),
                        ))
                        .child(mac_button(
                            "Always disown",
                            "toast-always",
                            false,
                            self.theme,
                            cx.listener(Self::always_disown_daemon),
                        ))
                        .child(mac_button(
                            "Quit",
                            "toast-quit",
                            false,
                            self.theme,
                            cx.listener(Self::quit_gui),
                        )),
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
            .items_center()
            .when(self.chat.cards.is_empty(), |view| {
                view.child(div().text_sm().text_color(self.theme.tertiary_text).child(
                    if self.chat.is_streaming() {
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
                    } => (self.theme.accent, "You", true, text.clone()),
                    Card::Message {
                        role: Role::Assistant,
                        text,
                    } => (self.theme.canvas, "Tau", false, text.clone()),
                    Card::Message {
                        role: Role::System,
                        text,
                    } => (
                        if text.starts_with("Request failed") {
                            self.theme.error_surface
                        } else {
                            self.theme.surface
                        },
                        "System",
                        false,
                        text.clone(),
                    ),
                    Card::Message {
                        role: Role::Tool,
                        text,
                    } => (self.theme.elevated, "Tool", false, text.clone()),
                    Card::Tool {
                        name,
                        input,
                        output,
                        expanded,
                    } => (
                        self.theme.elevated,
                        "Tool",
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
                        self.theme.success_surface,
                        "Changes",
                        false,
                        format!(
                            "{path} ({})\n{patch}",
                            if *approved { "approved" } else { "pending" }
                        ),
                    ),
                    Card::Error {
                        summary,
                        details,
                        expanded,
                    } => (
                        self.theme.error_surface,
                        "Couldn’t Complete Request",
                        false,
                        if *expanded {
                            format!("{summary}\n\nTechnical details\n{details}")
                        } else {
                            summary.clone()
                        },
                    ),
                    Card::Permission {
                        tool, description, ..
                    } => (
                        self.theme.elevated,
                        "Permission",
                        false,
                        format!("{tool}: {description}"),
                    ),
                    Card::Question {
                        question, answer, ..
                    } => (
                        self.theme.selection,
                        "Question",
                        false,
                        format!(
                            "{question}\n{}",
                            answer.as_deref().unwrap_or("awaiting answer")
                        ),
                    ),
                };
                let mut card_view = div()
                    .id(("card", index))
                    .debug_selector(|| format!("card-{index}"))
                    .w_full()
                    .max_w(px(760.))
                    .focusable()
                    .hover(|style| style.bg(self.theme.surface))
                    .flex()
                    .flex_col()
                    .when(align_end, |element| element.items_end())
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.secondary_text)
                            .child(label),
                    )
                    .child(feed_body(label, text).bg(background));
                if matches!(card, Card::Tool { .. }) {
                    card_view = card_view.on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |view, event, window, cx| {
                            view.toggle_tool(index, event, window, cx)
                        }),
                    );
                }
                if let Card::Error { expanded, .. } = card {
                    let expanded = *expanded;
                    card_view = card_view.child(
                        div()
                            .flex()
                            .gap_2()
                            .child(mac_button(
                                "Retry",
                                "error-retry",
                                true,
                                self.theme,
                                cx.listener(Self::retry_last_prompt),
                            ))
                            .child(mac_button(
                                if expanded {
                                    "Hide Details"
                                } else {
                                    "Show Details"
                                },
                                "error-toggle",
                                false,
                                self.theme,
                                cx.listener(move |view, event, window, cx| {
                                    view.toggle_error(index, event, window, cx)
                                }),
                            )),
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
                            .child(mac_button(
                                "Accept",
                                "diff-accept",
                                true,
                                self.theme,
                                cx.listener(move |view, event, window, cx| {
                                    view.choose_diff(index, true, event, window, cx)
                                }),
                            ))
                            .child(mac_button(
                                "Reject",
                                "diff-reject",
                                false,
                                self.theme,
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
                                (PermissionChoice::AllowOnce, "Allow once", true),
                                (PermissionChoice::AllowAlways, "Always", true),
                                (PermissionChoice::Reject, "Reject", false),
                                (PermissionChoice::Inspect, "Inspect", false),
                                (PermissionChoice::Cancel, "Cancel", false),
                            ]
                            .into_iter()
                            .map(|(choice, label, primary)| {
                                mac_button(
                                    label,
                                    "permission-choice",
                                    primary,
                                    self.theme,
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
                            .child(mac_button(
                                "Yes",
                                "question-yes",
                                true,
                                self.theme,
                                cx.listener(move |view, event, window, cx| {
                                    view.answer_question(index, "yes", event, window, cx)
                                }),
                            ))
                            .child(mac_button(
                                "No",
                                "question-no",
                                false,
                                self.theme,
                                cx.listener(move |view, event, window, cx| {
                                    view.answer_question(index, "no", event, window, cx)
                                }),
                            )),
                    );
                }
                card_view
            }))
            .children(
                typed_feed
                    .items
                    .iter()
                    .filter(|item| feed_item_is_pending(&self.chat, item))
                    .map(|item| {
                        let category = item.category;
                        // Policy ids live in the typed EventKind payload.  The
                        // envelope request id is a different correlation field and
                        // must never be substituted for it.
                        let request_id = feed_policy_request_id(item).unwrap_or_default();
                        let label = format!(
                            "{} {} · #{} · {}",
                            item.avatar, item.role_mark, item.sequence, item.timestamp
                        );
                        let mut item_view = div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .p_3()
                            .rounded_md()
                            .bg(self.theme.surface)
                            .border_1()
                            .border_color(self.theme.separator)
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(self.theme.tertiary_text)
                                    .child(label.clone()),
                            )
                            .child(
                                feed_body(item.role_mark, item.visible_detail().to_owned())
                                    .text_color(self.theme.text),
                            );
                        if item.actions.permission {
                            item_view = item_view.child(
                                div().flex().gap_2().children(
                                    [
                                        (PermissionChoice::AllowOnce, "Allow once", true),
                                        (PermissionChoice::Reject, "Reject", false),
                                    ]
                                    .into_iter()
                                    .map(
                                        |(choice, label, primary)| {
                                            let request_id = request_id.to_owned();
                                            mac_button(
                                                label,
                                                "feed-permission",
                                                primary,
                                                self.theme,
                                                cx.listener(move |view, event, window, cx| {
                                                    if let Some(index) = view
                                                        .card_index_for_request(
                                                            &request_id,
                                                            category,
                                                        )
                                                    {
                                                        view.choose_permission(
                                                            index, choice, event, window, cx,
                                                        );
                                                    }
                                                }),
                                            )
                                        },
                                    ),
                                ),
                            );
                        } else if item.actions.question {
                            item_view = item_view.child(div().flex().gap_2().children(
                                ["yes", "no"].into_iter().map(|answer| {
                                    let request_id = request_id.to_owned();
                                    mac_button(
                                        answer,
                                        "feed-question",
                                        answer == "yes",
                                        self.theme,
                                        cx.listener(move |view, event, window, cx| {
                                            if let Some(index) =
                                                view.card_index_for_request(&request_id, category)
                                            {
                                                view.answer_question(
                                                    index, answer, event, window, cx,
                                                );
                                            }
                                        }),
                                    )
                                }),
                            ));
                        } else if item.actions.diff {
                            item_view = item_view.child(
                                div().flex().gap_2().children(
                                    [(true, "Accept", true), (false, "Reject", false)]
                                        .into_iter()
                                        .map(|(approved, label, primary)| {
                                            let request_id = request_id.to_owned();
                                            mac_button(
                                                label,
                                                "feed-diff",
                                                primary,
                                                self.theme,
                                                cx.listener(move |view, event, window, cx| {
                                                    if let Some(index) = view
                                                        .card_index_for_request(
                                                            &request_id,
                                                            category,
                                                        )
                                                    {
                                                        view.choose_diff(
                                                            index, approved, event, window, cx,
                                                        );
                                                    }
                                                }),
                                            )
                                        }),
                                ),
                            );
                        }
                        item_view
                    }),
            );
        let sidebar = div()
            .id("inspector")
            .w(px(320.))
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .flex_none()
            .overflow_y_scroll()
            .border_l_1()
            .border_color(self.theme.separator)
            .bg(self.theme.sidebar)
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Inspector"),
            )
            .children(
                description
                    .sidebar
                    .iter()
                    .filter(|(_, value)| !value.is_empty() && value != "Unavailable")
                    .map(|(title, value)| sidebar_card(title, value, self.theme)),
            )
            .child(
                div()
                    .mt_2()
                    .pt_4()
                    .border_t_1()
                    .border_color(self.theme.separator)
                    .child(self.operations_panel(cx)),
            );
        let footer = div()
            .p_4()
            .flex()
            .justify_center()
            .bg(self.theme.toolbar)
            .border_t_1()
            .border_color(self.theme.separator)
            .child(
                div()
                    .w_full()
                    .max_w(px(900.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .rounded_xl()
                    .bg(self.theme.surface)
                    .border_1()
                    .border_color(self.theme.separator)
                    .shadow_sm()
                    .child(self.input.clone())
                    .when_some(
                        {
                            let input = self.input.read(cx);
                            let refs = input.file_references();
                            let char_count = input.char_count();
                            (!refs.is_empty() || char_count > 0).then(|| (refs, char_count))
                        },
                        |card, (refs, char_count)| {
                            card.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_xs()
                                    .text_color(self.theme.tertiary_text)
                                    .children(refs.into_iter().map(|reference| {
                                        div()
                                            .px_2()
                                            .py_1()
                                            .rounded_md()
                                            .bg(self.theme.elevated)
                                            .text_color(self.theme.secondary_text)
                                            .child(reference)
                                    }))
                                    .when(char_count > 0, |div| {
                                        div.child(format!("{} characters", char_count))
                                    }),
                            )
                        },
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .id("attach-button")
                                    .debug_selector(|| "attach-button".into())
                                    .h(px(28.))
                                    .w(px(28.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .bg(self.theme.elevated)
                                    .text_color(self.theme.secondary_text)
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .hover(|style| {
                                        style.bg(self.theme.surface).text_color(self.theme.text)
                                    })
                                    .cursor_pointer()
                                    .on_mouse_up(MouseButton::Left, cx.listener(Self::click_model))
                                    .child("+"),
                            )
                            .child(composer_pill(
                                &format!(
                                    "^ Agent · {}",
                                    if self.agent.is_empty() {
                                        "default"
                                    } else {
                                        &self.agent
                                    }
                                ),
                                "agent-button",
                                self.theme,
                                cx.listener(Self::click_agent),
                            ))
                            .child(composer_pill(
                                &format!("^ Model · {}", current_model),
                                "model-button",
                                self.theme,
                                cx.listener(Self::click_model),
                            ))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .id("send-button")
                                    .debug_selector(|| "send-button".into())
                                    .w(px(28.))
                                    .h(px(28.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(self.theme.accent)
                                    .hover(|style| style.bg(self.theme.accent_hover))
                                    .focusable()
                                    .text_color(rgb(0xffffff))
                                    .rounded_full()
                                    .cursor_pointer()
                                    .when(send_blocked, |button| button.opacity(0.45))
                                    .on_mouse_up(MouseButton::Left, cx.listener(Self::click_send))
                                    .child("→"),
                            ),
                    ),
            )
            .when(self.chat.is_streaming(), |footer| {
                footer.child(
                    div()
                        .id("cancel-button")
                        .absolute()
                        .bottom(px(16.))
                        .right(px(16.))
                        .px_3()
                        .h(px(32.))
                        .flex()
                        .items_center()
                        .bg(self.theme.error_surface)
                        .hover(|style| style.bg(self.theme.error_text))
                        .focusable()
                        .text_color(self.theme.error_text)
                        .rounded_md()
                        .cursor_pointer()
                        .on_mouse_up(MouseButton::Left, cx.listener(Self::cancel_turn))
                        .child("Cancel"),
                )
            });
        let mut root = div()
            .size_full()
            .bg(self.theme.canvas)
            .text_color(self.theme.text)
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
            root = root.child(deferred(self.picker_overlay(cx, kind)).with_priority(10));
        } else if command_active {
            root = root
                .child(deferred(self.picker_overlay(cx, PickerKind::Command)).with_priority(10));
        }
        root = root.on_action(cx.listener(Self::toggle_sidebar));
        root = root.on_action(cx.listener(Self::toggle_follow));
        root = root.on_action(cx.listener(Self::operations_status_action));
        root = root.on_action(cx.listener(Self::operations_git_action));
        root = root.on_action(cx.listener(Self::operations_changes_action));
        root = root.on_action(cx.listener(Self::operations_refresh_action));
        root = root.on_action(cx.listener(Self::operations_stage_action));
        root = root.on_action(cx.listener(Self::operations_unstage_action));
        root = root.on_action(cx.listener(Self::operations_revert_action));
        root = root.on_action(cx.listener(Self::operations_keep_action));
        root = root.on_action(cx.listener(Self::operations_acknowledge_action));
        root = root.on_action(cx.listener(Self::operations_create_action));
        root = root.on_action(cx.listener(Self::operations_switch_action));
        let center_column = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .child(header)
            .when(self.sessions_open, |col| col.child(self.session_panel(cx)))
            .children(runtime_banner_view)
            .child(transcript)
            .child(footer);

        let main_body = div()
            .flex()
            .flex_1()
            .min_h(px(0.))
            .child(center_column)
            .when(self.sidebar_visible, |view| view.child(sidebar));

        root.child(main_body)
            .when(self.toast_visible, |root| {
                root.child(
                    deferred(toast.absolute().top(px(0.)).left(px(0.)).right(px(0.)))
                        .with_priority(5),
                )
            })
            .when(self.account_sheet, |root| {
                root.child(deferred(self.account_sheet(cx)).with_priority(20))
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

fn sidebar_card(title: &str, value: &str, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .rounded_md()
        .bg(theme.surface)
        .border_1()
        .border_color(theme.separator)
        .child(
            div()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.tertiary_text)
                .child(title.to_string()),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.text)
                .child(value.to_string()),
        )
}

fn mac_button<T>(
    label: impl Into<String>,
    selector: &'static str,
    primary: bool,
    theme: Theme,
    callback: T,
) -> impl IntoElement
where
    T: Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
{
    div()
        .debug_selector(|| selector.into())
        .h(px(32.))
        .px_3()
        .flex()
        .items_center()
        .rounded_md()
        .bg(if primary {
            theme.accent
        } else {
            theme.elevated
        })
        .text_color(if primary { rgb(0xffffff) } else { theme.text })
        .hover(move |style| {
            style.bg(if primary {
                theme.accent_hover
            } else {
                theme.surface
            })
        })
        .cursor_pointer()
        .on_mouse_up(MouseButton::Left, callback)
        .child(label.into())
}

fn toast_button<T>(label: impl Into<String>, color: u32, callback: T) -> impl IntoElement
where
    T: Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
{
    div()
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(rgb(color))
        .hover(|style| style.opacity(0.9))
        .cursor_pointer()
        .on_mouse_up(MouseButton::Left, callback)
        .child(label.into())
}

fn testable_button<T>(
    label: impl Into<String>,
    color: u32,
    selector: &'static str,
    callback: T,
) -> impl IntoElement
where
    T: Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
{
    div()
        .debug_selector(|| selector.into())
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(rgb(color))
        .hover(|style| style.opacity(0.9))
        .cursor_pointer()
        .on_mouse_up(MouseButton::Left, callback)
        .child(label.into())
}

fn composer_pill<T>(
    label: impl Into<String>,
    selector: &'static str,
    theme: Theme,
    callback: T,
) -> impl IntoElement
where
    T: Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
{
    div()
        .debug_selector(|| selector.into())
        .h(px(28.))
        .px_3()
        .flex()
        .items_center()
        .rounded_md()
        .bg(theme.elevated)
        .text_color(theme.secondary_text)
        .text_xs()
        .hover(move |style| style.bg(theme.surface))
        .cursor_pointer()
        .on_mouse_up(MouseButton::Left, callback)
        .child(label.into())
}

#[cfg(test)]
mod view_model_tests {
    use super::*;
    use gpui::{Modifiers, TestAppContext, VisualTestContext};

    fn test_backend(runtime: &tokio::runtime::Runtime) -> Backend {
        Backend::from_parts(
            PathBuf::from("/tmp/tau-gui-click-test.sock"),
            runtime.handle().clone(),
            None,
            "/tmp/tau-gui-click-project".into(),
            "test/model".into(),
        )
    }

    fn click(cx: &mut VisualTestContext, selector: &'static str) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("missing rendered control: {selector}"));
        cx.simulate_click(bounds.center(), Modifiers::none());
    }

    #[gpui::test]
    fn rendered_root_controls_dispatch_mouse_clicks(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let backend = test_backend(&runtime);
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = TauView::new(backend, cx);
            view.sidebar_visible = true;
            view
        });

        click(cx, "operations-git");
        assert_eq!(
            cx.update(|_, app| view.read(app).operations_tab),
            OperationsTab::Git
        );

        click(cx, "operations-refresh");
        assert!(
            cx.update(|_, app| view.read(app).operations_error.clone())
                .is_some()
        );

        click(cx, "new-chat-button");
        assert!(matches!(
            cx.update(|_, app| view.read(app).runtime.clone()),
            RuntimeState::Failed(_)
        ));

        click(cx, "sessions-button");
        assert!(cx.update(|_, app| view.read(app).sessions_open));

        click(cx, "follow-button");
        assert!(!cx.update(|_, app| view.read(app).follow_output));

        click(cx, "sidebar-button");
        assert!(!cx.update(|_, app| view.read(app).sidebar_visible));

        click(cx, "model-button");
        assert_eq!(
            cx.update(|_, app| view.read(app).picker),
            Some(PickerKind::Model)
        );
    }

    #[gpui::test]
    fn rendered_tool_card_click_toggles_inspector(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let backend = test_backend(&runtime);
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut view = TauView::new(backend, cx);
            view.chat.cards.push(Card::Tool {
                name: "read".into(),
                input: "file.txt".into(),
                output: "contents".into(),
                expanded: false,
            });
            view
        });

        click(cx, "card-0");
        assert!(matches!(
            &cx.update(|_, app| view.read(app).chat.cards[0].clone()),
            Card::Tool { expanded: true, .. }
        ));
    }

    #[gpui::test]
    fn rendered_account_sheet_opens_and_copies_explicit_oauth_link(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let backend = test_backend(&runtime);
        let authorize_url = "https://auth.openai.com/oauth/authorize?state=stable";
        {
            let (_view, window) = cx.add_window_view(|_, cx| {
                let mut view = TauView::new(backend, cx);
                view.account_sheet = true;
                view.auth_status = AuthState::Pending {
                    authorize_url: authorize_url.into(),
                };
                view
            });
            click(window, "account-open-browser");
            click(window, "account-copy-link");
        }
        assert_eq!(cx.opened_url().as_deref(), Some(authorize_url));
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some(authorize_url)
        );
    }

    #[gpui::test]
    fn repository_operations_use_opaque_project_id_not_root_path(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let backend = test_backend(&runtime);
        let (view, window) = cx.add_window_view(|_, cx| TauView::new(backend, cx));
        view.update(window, |view, cx| {
            view.select_project(
                Some("project-opaque-id".into()),
                Some(PathBuf::from("/workspace/project")),
                cx,
            );
            assert_eq!(
                view.operation_project().as_deref(),
                Some("project-opaque-id")
            );
        });
    }

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

    #[test]
    fn feed_controls_use_policy_ids_not_envelope_request_ids() {
        let raw = tau_proto::prelude::TurnEvent::PermissionRequested {
            turn_id: "turn".into(),
            request_id: "policy-42".into(),
            tool: "shell".into(),
            description: "run".into(),
        };
        let event = crate::chat::TypedEvent {
            event_id: "event".into(),
            session_id: "session".into(),
            sequence: 1,
            occurred_at: 0,
            request_id: Some("envelope-99".into()),
            turn_id: "turn".into(),
            kind: EventKind::PermissionRequested {
                permission_id: "policy-42".into(),
                tool: "shell".into(),
                description: "run".into(),
            },
            raw,
        };
        let item = crate::feed::project(&event, crate::feed::DEFAULT_COLLAPSE_AT);
        assert_eq!(feed_policy_request_id(&item), Some("policy-42"));
        assert_ne!(feed_policy_request_id(&item), item.request_id.as_deref());
    }

    #[test]
    fn acknowledged_feed_policy_controls_are_removed_from_the_root() {
        let raw = tau_proto::prelude::TurnEvent::QuestionAsked {
            turn_id: "turn".into(),
            question_id: "question-7".into(),
            question: "continue?".into(),
        };
        let event = crate::chat::TypedEvent {
            event_id: "event".into(),
            session_id: "session".into(),
            sequence: 1,
            occurred_at: 0,
            request_id: None,
            turn_id: "turn".into(),
            kind: EventKind::QuestionAsked {
                question_id: "question-7".into(),
                question: "continue?".into(),
            },
            raw,
        };
        let item = crate::feed::project(&event, crate::feed::DEFAULT_COLLAPSE_AT);
        let mut chat = ChatState::default();
        assert!(feed_item_is_pending(&chat, &item));
        chat.policy_states
            .insert("question-7".into(), crate::chat::PolicyState::Ack);
        assert!(!feed_item_is_pending(&chat, &item));
    }
}
