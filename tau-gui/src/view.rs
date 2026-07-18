use gpui::{
    App, Context, Entity, Focusable, KeyBinding, MouseButton, MouseUpEvent, Render, ScrollHandle,
    Task, Window, div, prelude::*, px, rgb,
};
use tau_client::TurnStreamEvent;
use tau_proto::prelude::{ClientResponse, QuestionAnswer, TurnPermissionChoice};

use crate::backend::{Backend, DaemonAction};
use crate::chat::{Card, ChatAction, ChatState, ChatStatus, EventKind, PermissionChoice, Role};
use crate::input::TextInput;

gpui::actions!(
    tau_view,
    [Submit, SwitchAgent, CycleModel, ToggleSidebar, ToggleFollow]
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
    ]);
}

#[derive(Clone, Copy)]
enum AgentMode {
    Plan,
    Code,
}

impl AgentMode {
    fn label(self) -> &'static str {
        match self {
            Self::Plan => "Plan",
            Self::Code => "Code",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Plan => Self::Code,
            Self::Code => Self::Plan,
        }
    }
}

pub struct TauView {
    input: Entity<TextInput>,
    backend: Backend,
    chat: ChatState,
    task: Option<Task<()>>,
    agent: AgentMode,
    toast_visible: bool,
    model_index: usize,
    models: Vec<String>,
    sidebar_visible: bool,
    transcript_scroll: ScrollHandle,
    follow_output: bool,
}

impl TauView {
    pub fn new(backend: Backend, cx: &mut Context<Self>) -> Self {
        let input = cx.new(TextInput::new);
        let toast_visible = backend.auto_started();
        let preferences = crate::preferences::path()
            .ok()
            .and_then(|path| crate::preferences::GuiPreferences::load_from(&path).ok())
            .unwrap_or_default();
        let models = preferences
            .favorites
            .iter()
            .chain(preferences.recent_models.iter())
            .cloned()
            .collect();
        Self {
            input,
            backend,
            chat: ChatState::default(),
            task: None,
            agent: AgentMode::Code,
            toast_visible,
            model_index: 0,
            models,
            sidebar_visible: preferences.sidebar,
            transcript_scroll: ScrollHandle::new(),
            follow_output: true,
        }
    }

    fn submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        self.send(window, cx);
    }

    fn click_send(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.send(window, cx);
    }

    fn switch_agent(&mut self, _: &SwitchAgent, _: &mut Window, cx: &mut Context<Self>) {
        self.agent = self.agent.next();
        self.chat.status = ChatStatus::Ready;
        cx.notify();
    }

    fn cycle_model(&mut self, _: &CycleModel, _: &mut Window, cx: &mut Context<Self>) {
        if !self.models.is_empty() {
            self.model_index = (self.model_index + 1) % self.models.len();
        }
        cx.notify();
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
                Card::Diff { path, .. } => Some((
                    self.chat
                        .events
                        .iter()
                        .rev()
                        .find_map(|event| match &event.kind {
                            EventKind::DiffRequested {
                                request_id: candidate,
                                path: event_path,
                                ..
                            } if event_path == path => Some(candidate.clone()),
                            _ => None,
                        }),
                    path.clone(),
                )),
                _ => None,
            })
            .unwrap_or_default();
        self.chat.reduce(ChatAction::ApproveDiff(index, approved));
        self.backend.respond(ClientResponse::DiffHunk {
            request_id: request_id.unwrap_or_default(),
            path,
            index: index as u32,
            approved,
        });
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
        let choice = match choice {
            PermissionChoice::AllowOnce => TurnPermissionChoice::AllowOnce,
            PermissionChoice::AllowAlways => TurnPermissionChoice::AllowAlways,
            PermissionChoice::Reject => TurnPermissionChoice::Reject,
            PermissionChoice::Inspect => TurnPermissionChoice::Inspect,
            PermissionChoice::Cancel => TurnPermissionChoice::Cancel,
        };
        self.backend.respond(ClientResponse::Permission {
            request_id: request_id.unwrap_or_default(),
            choice,
        });
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
        self.backend.respond(ClientResponse::Question {
            question_id: question_id.unwrap_or_default(),
            answer: QuestionAnswer(answer.into()),
        });
        cx.notify();
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
        self.input.update(cx, |input, _| input.reset());
        self.chat.reduce(ChatAction::Submit(prompt.clone()));
        cx.notify();
        window.focus(&self.input.focus_handle(cx));

        let selected_model = self.models.get(self.model_index).cloned();
        let receiver = self.backend.turn_with_options(
            prompt,
            self.chat.session_id.clone(),
            Some(self.agent.label().into()),
            selected_model,
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

    fn apply_event(&mut self, event: Result<TurnStreamEvent, String>, cx: &mut Context<Self>) {
        match event {
            Ok(TurnStreamEvent::Event(event)) => {
                self.chat.reduce(ChatAction::SessionEvent(event));
                cx.notify();
            }
            Ok(TurnStreamEvent::Complete(_)) => {
                self.chat.active_assistant = None;
                self.chat.status = ChatStatus::Ready;
                cx.notify();
            }
            Err(error) => {
                self.chat.reduce(ChatAction::Error(error));
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
        let description = describe_chat(
            &self.chat,
            self.agent.label(),
            current_model,
            self.backend.cwd(),
        );
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
            .child(div().text_sm().text_color(rgb(0x8c96a8)).child(format!(
                "{}  ·  {}",
                current_model,
                match &self.chat.status {
                    ChatStatus::Ready => "Ready",
                    ChatStatus::Streaming => "Thinking...",
                    ChatStatus::Failed(_) => "Request failed",
                }
            )));
        let toast = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .p_3()
            .bg(rgb(0x463b24))
            .text_color(rgb(0xf3d28a))
            .child("tau started the daemon automatically for this GUI session")
            .child(
                div()
                    .flex()
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
                    .flex()
                    .flex_col()
                    .when(align_end, |element| element.items_end())
                    .child(div().text_xs().text_color(rgb(0x8994a8)).child(label))
                    .child(
                        div()
                            .flex()
                            .max_w(px(820.))
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
            .border_l_1()
            .border_color(rgb(0x2c3340))
            .children(
                description
                    .sidebar
                    .iter()
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
                            .px_4()
                            .py_3()
                            .bg(rgb(0x85b8ff))
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
                        .px_3()
                        .py_2()
                        .bg(rgb(0x7d3b3b))
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
            .key_context("TauView")
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::switch_agent));
        root = root.on_action(cx.listener(Self::cycle_model));
        root = root.on_action(cx.listener(Self::toggle_sidebar));
        root = root.on_action(cx.listener(Self::toggle_follow));
        if self.toast_visible {
            root = root.child(toast);
        }
        root.child(header)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .child(transcript)
                    .when(self.sidebar_visible, |view| view.child(sidebar)),
            )
            .child(footer)
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
