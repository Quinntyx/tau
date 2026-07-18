use gpui::{
    App, Context, Entity, Focusable, KeyBinding, MouseButton, MouseUpEvent, Render,
    StatefulInteractiveElement, Task, Window, div, prelude::*, px, rgb,
};
use tau_client::TurnStreamEvent;

use crate::backend::{Backend, DaemonAction};
use crate::chat::{Card, ChatAction, ChatState, ChatStatus, PermissionChoice, Role};
use crate::input::TextInput;

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
    agent: AgentMode,
    toast_visible: bool,
    model_index: usize,
    models: Vec<String>,
    sidebar_visible: bool,
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

    fn choose_diff(
        &mut self,
        index: usize,
        approved: bool,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.chat.reduce(ChatAction::ApproveDiff(index, approved));
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
        self.chat.reduce(ChatAction::Permission(index, choice));
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
        self.chat
            .reduce(ChatAction::AnswerQuestion(index, answer.into()));
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
        self.input.update(cx, |input, _| input.set_disabled(false));
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
        self.input.update(cx, |input, _| {
            input.record_submission(prompt.clone());
            input.reset();
            input.set_disabled(true);
        });
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
                self.input.update(cx, |input, _| input.set_disabled(false));
                cx.notify();
            }
            Err(error) => {
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
            .flex_1()
            .min_h(px(0.))
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
            }));
        let sidebar = div()
            .w(px(260.))
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .flex_none()
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
        root.child(header)
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
