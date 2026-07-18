use gpui::{
    App, ClipboardItem, Context, Entity, Focusable, KeyBinding, MouseButton, MouseUpEvent, Render,
    Task, Window, div, prelude::*, px, rgb,
};
use tau_client::TurnStreamEvent;

use crate::backend::{Backend, DaemonAction};
use crate::chat::{Card, ChatAction, ChatState, ChatStatus, PermissionChoice, PolicyRequest, Role};
use crate::input::TextInput;
use tau_proto::prelude::{
    PermissionChoice as WirePermissionChoice, PermissionScope, TurnPermissionChoice,
};

gpui::actions!(tau_view, [Submit, SwitchAgent, CycleModel]);

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
    _policy_task: Option<Task<()>>,
    ack_tasks: Vec<Task<()>>,
    agent: AgentMode,
    toast_visible: bool,
    model_index: usize,
    models: Vec<String>,
    sidebar_visible: bool,
    inspector: Option<String>,
    inspector_index: Option<usize>,
}

impl TauView {
    pub fn new(backend: Backend, cx: &mut Context<Self>) -> Self {
        let input = cx.new(TextInput::new);
        let mut policy_events = backend.policy_events();
        let policy_task = cx.spawn(async move |this, cx| {
            use futures_util::StreamExt;
            while let Some(event) = policy_events.next().await {
                let _ = this.update(cx, |view, cx| {
                    view.chat.reduce(ChatAction::PolicyEvent(event));
                    cx.notify();
                });
            }
        });
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
            _policy_task: Some(policy_task),
            ack_tasks: Vec::new(),
            agent: AgentMode::Code,
            toast_visible,
            model_index: 0,
            models,
            sidebar_visible: preferences.sidebar,
            inspector: None,
            inspector_index: None,
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

    /// Right-clicking a tool opens a non-mutating view of the exact data held
    /// by the transcript.  Keeping this separate from expansion makes the
    /// inspector useful when a tool card is collapsed.
    fn inspect_tool(
        &mut self,
        index: usize,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(Card::Tool {
            name,
            input,
            output,
            ..
        }) = self.chat.cards.get(index)
        {
            self.inspector = Some(raw_tool_json(name, input, output));
            self.inspector_index = Some(index);
            cx.notify();
        }
    }

    fn copy_tool(
        &mut self,
        index: usize,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(raw) = self.chat.cards.get(index).and_then(|card| match card {
            Card::Tool {
                name,
                input,
                output,
                ..
            } => Some(raw_tool_json(name, input, output)),
            _ => None,
        }) {
            cx.write_to_clipboard(ClipboardItem::new_string(raw));
            self.inspector = Some("Copied raw tool JSON to clipboard.".into());
            cx.notify();
        }
    }

    fn choose_diff(
        &mut self,
        index: usize,
        approved: bool,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Card::Diff {
            request_id, path, ..
        }) = self.chat.cards.get(index).cloned()
        else {
            return;
        };
        self.chat.reduce(ChatAction::ApproveDiff(index, approved));
        if let Some(PolicyRequest::Diff(request)) =
            self.chat.policy_requests.get(&request_id).cloned()
        {
            let decisions = request
                .files
                .iter()
                .flat_map(|file| {
                    file.hunks
                        .iter()
                        .map(|hunk| tau_proto::prelude::HunkDecision {
                            file: file.path.clone(),
                            hunk: Some(hunk.id.clone()),
                            accept: approved,
                        })
                })
                .collect();
            let receiver = self.backend.reply_diff(request, approved, decisions);
            self.watch_policy_ack(request_id, receiver, cx);
        } else if let Some((session_id, turn_id)) =
            self.chat.request_context.get(&request_id).cloned()
        {
            let receiver = self
                .backend
                .reply_turn_diff(session_id, turn_id, path, approved);
            self.watch_turn_ack(request_id, receiver, cx);
        }
        cx.notify();
    }

    fn choose_diff_hunk(
        &mut self,
        index: usize,
        hunk_index: usize,
        approved: bool,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Card::Diff { request_id, .. }) = self.chat.cards.get(index).cloned() else {
            return;
        };
        let Some(PolicyRequest::Diff(request)) =
            self.chat.policy_requests.get(&request_id).cloned()
        else {
            return;
        };
        let Some((file_path, hunk_id)) = request
            .files
            .iter()
            .flat_map(|file| file.hunks.iter().map(move |hunk| (file, hunk)))
            .nth(hunk_index)
            .map(|(file, hunk)| (file.path.clone(), hunk.id.clone()))
        else {
            return;
        };
        self.chat.reduce(ChatAction::DiffHunk {
            request_id: request_id.clone(),
            hunk: hunk_index,
            approved,
        });
        let receiver = self.backend.decide_diff(
            request,
            vec![tau_proto::prelude::HunkDecision {
                file: file_path,
                hunk: Some(hunk_id),
                accept: approved,
            }],
        );
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
        let Some(Card::Permission {
            request_id,
            tool,
            description,
        }) = self.chat.cards.get(index).cloned()
        else {
            return;
        };
        self.chat.reduce(ChatAction::Permission(index, choice));
        if matches!(choice, PermissionChoice::Inspect) {
            // Inspect is deliberately local: it must not acknowledge the
            // request, but it still needs a visible result rather than being
            // a no-op control.
            self.inspector = Some(format!(
                "Permission request\n\ntool: {tool}\nrequest_id: {request_id}\ndetails: {description}"
            ));
            self.inspector_index = None;
            cx.notify();
            return;
        }
        if let Some(PolicyRequest::Permission(request)) =
            self.chat.policy_requests.get(&request_id).cloned()
        {
            let (wire_choice, scope) = match choice {
                PermissionChoice::AllowOnce => (WirePermissionChoice::Allow, PermissionScope::Once),
                PermissionChoice::AllowAlways => {
                    (WirePermissionChoice::Allow, PermissionScope::Global)
                }
                PermissionChoice::Reject | PermissionChoice::Cancel => {
                    (WirePermissionChoice::Reject, PermissionScope::Once)
                }
                PermissionChoice::Inspect => unreachable!(),
            };
            let receiver = self.backend.reply_permission(request, wire_choice, scope);
            self.watch_policy_ack(request_id, receiver, cx);
        } else if let Some((session_id, turn_id)) =
            self.chat.request_context.get(&request_id).cloned()
        {
            let wire_choice = match choice {
                PermissionChoice::AllowOnce => TurnPermissionChoice::AllowOnce,
                PermissionChoice::AllowAlways => TurnPermissionChoice::AllowAlways,
                PermissionChoice::Reject => TurnPermissionChoice::Reject,
                PermissionChoice::Cancel => TurnPermissionChoice::Cancel,
                PermissionChoice::Inspect => unreachable!(),
            };
            let receiver = self.backend.reply_turn_permission(
                session_id,
                turn_id,
                request_id.clone(),
                wire_choice,
            );
            self.watch_turn_ack(request_id, receiver, cx);
        }
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
        let Some(Card::Question { question_id, .. }) = self.chat.cards.get(index).cloned() else {
            return;
        };
        self.chat
            .reduce(ChatAction::AnswerQuestion(index, answer.into()));
        if let Some(PolicyRequest::Question(request)) =
            self.chat.policy_requests.get(&question_id).cloned()
        {
            let receiver = self.backend.reply_question(request, answer.into());
            self.watch_policy_ack(question_id, receiver, cx);
        } else if let Some((session_id, turn_id)) =
            self.chat.request_context.get(&question_id).cloned()
        {
            let receiver = self.backend.reply_turn_question(
                session_id,
                turn_id,
                question_id.clone(),
                answer.into(),
            );
            self.watch_turn_ack(question_id, receiver, cx);
        }
        cx.notify();
    }

    fn choose_plan(
        &mut self,
        index: usize,
        accepted: bool,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Card::Plan {
            request_id,
            revision,
            ..
        }) = self.chat.cards.get(index).cloned()
        else {
            return;
        };
        let Some(PolicyRequest::Plan(request)) =
            self.chat.policy_requests.get(&request_id).cloned()
        else {
            return;
        };
        self.chat.reduce(ChatAction::PolicyDecision {
            request_id: request_id.clone(),
            choice: accepted.to_string(),
        });
        self.watch_policy_ack(
            request_id,
            self.backend.reply_plan(request, accepted, revision),
            cx,
        );
        cx.notify();
    }

    fn choose_airtight(
        &mut self,
        index: usize,
        granted: bool,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Card::Airtight { request_id, .. }) = self.chat.cards.get(index).cloned() else {
            return;
        };
        let Some(PolicyRequest::Airtight(request)) =
            self.chat.policy_requests.get(&request_id).cloned()
        else {
            return;
        };
        self.chat.reduce(ChatAction::PolicyDecision {
            request_id: request_id.clone(),
            choice: granted.to_string(),
        });
        self.watch_policy_ack(
            request_id,
            self.backend.reply_airtight(request, granted),
            cx,
        );
        cx.notify();
    }

    fn watch_policy_ack(
        &mut self,
        request_id: String,
        receiver: tokio::sync::oneshot::Receiver<Result<serde_json::Value, String>>,
        cx: &mut Context<Self>,
    ) {
        self.ack_tasks.push(cx.spawn(async move |this, cx| {
            let result = receiver
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result);
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(_) => {
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
        }));
    }

    fn watch_turn_ack(
        &mut self,
        request_id: String,
        receiver: tokio::sync::oneshot::Receiver<
            Result<tau_proto::prelude::TurnResponseResult, String>,
        >,
        cx: &mut Context<Self>,
    ) {
        self.ack_tasks.push(cx.spawn(async move |this, cx| {
            let result = receiver
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result);
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(_) => {
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
        }));
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
            .flex_1()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_3()
            .p_5()
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
                    Card::Plan {
                        revision, accepted, ..
                    } => (
                        0x293b32,
                        "plan",
                        false,
                        format!(
                            "revision {revision}\n{}",
                            accepted.map_or("awaiting decision", |value| if value {
                                "accepted"
                            } else {
                                "rejected"
                            })
                        ),
                    ),
                    Card::Airtight { step, granted, .. } => (
                        0x463b24,
                        "airtight",
                        false,
                        format!(
                            "step {step}\n{}",
                            granted.map_or("awaiting decision", |value| if value {
                                "granted"
                            } else {
                                "rejected"
                            })
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
                let request_id = match card {
                    Card::Permission { request_id, .. }
                    | Card::Diff { request_id, .. }
                    | Card::Plan { request_id, .. }
                    | Card::Airtight { request_id, .. } => Some(request_id.as_str()),
                    Card::Question { question_id, .. } => Some(question_id.as_str()),
                    _ => None,
                };
                if let Some(request_id) = request_id {
                    if let Some(state) = self.chat.policy_states.get(request_id) {
                        let status = match state {
                            crate::chat::PolicyState::Pending => "Sending…".to_string(),
                            crate::chat::PolicyState::Ack => "Acknowledged".to_string(),
                            crate::chat::PolicyState::Error(message) => {
                                format!("Error: {message} (choose again to retry)")
                            }
                        };
                        card_view = card_view
                            .child(div().text_xs().text_color(rgb(0xf3d28a)).child(status));
                    }
                }
                if matches!(card, Card::Tool { .. }) {
                    card_view = card_view.on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |view, event, window, cx| {
                            view.toggle_tool(index, event, window, cx)
                        }),
                    );
                    card_view = card_view.on_mouse_up(
                        MouseButton::Right,
                        cx.listener(move |view, event, window, cx| {
                            view.inspect_tool(index, event, window, cx)
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
                    if let Card::Diff { request_id, .. } = card {
                        if let Some(PolicyRequest::Diff(request)) =
                            self.chat.policy_requests.get(request_id)
                        {
                            for hunk_index in
                                0..request.files.iter().map(|file| file.hunks.len()).sum()
                            {
                                card_view = card_view.child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .child(toast_button(
                                            &format!("Hunk {hunk_index} accept"),
                                            0x39734a,
                                            cx.listener(move |view, event, window, cx| {
                                                view.choose_diff_hunk(
                                                    index, hunk_index, true, event, window, cx,
                                                )
                                            }),
                                        ))
                                        .child(toast_button(
                                            &format!("Hunk {hunk_index} reject"),
                                            0x7d3b3b,
                                            cx.listener(move |view, event, window, cx| {
                                                view.choose_diff_hunk(
                                                    index, hunk_index, false, event, window, cx,
                                                )
                                            }),
                                        )),
                                );
                            }
                        }
                    }
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
                if matches!(card, Card::Plan { accepted: None, .. }) {
                    card_view = card_view.child(
                        div().flex().gap_2().children(
                            [(true, "Accept", 0x39734a), (false, "Reject", 0x7d3b3b)]
                                .into_iter()
                                .map(|(accepted, label, color)| {
                                    toast_button(
                                        label,
                                        color,
                                        cx.listener(move |view, event, window, cx| {
                                            view.choose_plan(index, accepted, event, window, cx)
                                        }),
                                    )
                                }),
                        ),
                    );
                }
                if matches!(card, Card::Airtight { granted: None, .. }) {
                    card_view = card_view.child(
                        div().flex().gap_2().children(
                            [(true, "Grant", 0x39734a), (false, "Reject", 0x7d3b3b)]
                                .into_iter()
                                .map(|(granted, label, color)| {
                                    toast_button(
                                        label,
                                        color,
                                        cx.listener(move |view, event, window, cx| {
                                            view.choose_airtight(index, granted, event, window, cx)
                                        }),
                                    )
                                }),
                        ),
                    );
                }
                card_view
            }));
        let inspector = self.inspector.as_ref().map(|raw| {
            let mut panel = div().p_3().bg(rgb(0x202630)).child(raw.clone());
            if let Some(index) = self.inspector_index {
                panel = panel.child(toast_button(
                    "Copy raw JSON",
                    0x52627a,
                    cx.listener(move |view, event, window, cx| {
                        view.copy_tool(index, event, window, cx)
                    }),
                ));
            }
            panel
        });
        let sidebar = div()
            .w(px(260.))
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .border_l_1()
            .border_color(rgb(0x2c3340))
            .child(sidebar_card("AGENT", self.agent.label()))
            .child(sidebar_card(
                "PLAN",
                if self.chat.status == ChatStatus::Streaming {
                    "Active"
                } else {
                    "No active plan"
                },
            ))
            .child(sidebar_card("LSP", "Connected"))
            .child(sidebar_card(
                "TOKENS",
                &self
                    .chat
                    .usage
                    .map(|v| format!("{v} tokens"))
                    .unwrap_or_else(|| "No usage yet".into()),
            ))
            .child(sidebar_card(
                "SESSION",
                self.chat.session_id.as_deref().unwrap_or("Not started"),
            ))
            .child(sidebar_card("DIRECTORY", self.backend.cwd()));
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
        if self.toast_visible {
            root = root.child(toast);
        }
        if let Some(inspector) = inspector {
            root = root.child(inspector);
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

fn raw_tool_json(name: &str, input: &str, output: &str) -> String {
    format!(
        "{{\n  \"tool\": {},\n  \"input\": {},\n  \"output\": {}\n}}",
        serde_json::to_string(name).unwrap_or_default(),
        serde_json::to_string(input).unwrap_or_default(),
        serde_json::to_string(output).unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::raw_tool_json;

    #[test]
    fn raw_tool_json_is_inspectable_and_copyable() {
        let raw = raw_tool_json("shell", r#"{"cmd":"pwd"}"#, "ok");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("valid inspector JSON");
        assert_eq!(value["tool"], "shell");
        assert_eq!(value["input"], r#"{"cmd":"pwd"}"#);
        assert_eq!(value["output"], "ok");
    }
}
