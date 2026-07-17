use gpui::{
    App, Context, Entity, Focusable, KeyBinding, MouseButton, MouseUpEvent, Render, Task, Window,
    div, prelude::*, px, rgb,
};
use tau_client::TurnStreamEvent;

use crate::backend::{Backend, DaemonAction};
use crate::chat::{Card, ChatAction, ChatState, ChatStatus, Role};
use crate::input::TextInput;

gpui::actions!(tau_view, [Submit, SwitchAgent]);

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
}

impl TauView {
    pub fn new(backend: Backend, cx: &mut Context<Self>) -> Self {
        let input = cx.new(TextInput::new);
        let toast_visible = backend.auto_started();
        Self {
            input,
            backend,
            chat: ChatState::default(),
            task: None,
            agent: AgentMode::Code,
            toast_visible,
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

    fn hide_toast(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
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

        let receiver = self.backend.turn(prompt, self.chat.session_id.clone());
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
                self.backend.model(),
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
                        cx.listener(Self::hide_toast),
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
            .children(self.chat.cards.iter().map(|card| {
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
                    Card::Tool { name, output, .. } => {
                        (0x3b3425, "tool", false, format!("{name}\n{output}"))
                    }
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
                    Card::Permission { tool, description } => (
                        0x463b24,
                        "permission",
                        false,
                        format!("{tool}: {description}"),
                    ),
                };
                div()
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
                    )
            }));
        let sidebar = div()
            .w(px(260.))
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .border_l_1()
            .border_color(rgb(0x2c3340))
            .child(sidebar_card("AGENT", self.agent.label()))
            .child(sidebar_card("PLAN", "No active plan"))
            .child(sidebar_card("LSP", "No diagnostics"))
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
        if self.toast_visible {
            root = root.child(toast);
        }
        root.child(header)
            .child(div().flex().flex_1().child(transcript).child(sidebar))
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
