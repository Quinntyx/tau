use gpui::{
    App, Context, Entity, Focusable, KeyBinding, MouseButton, MouseUpEvent, Render, Task, Window,
    div, prelude::*, px, rgb,
};
use tau_client::CompletionEvent;
use tau_proto::prelude::CompletionStreamResult;

use crate::backend::Backend;
use crate::input::TextInput;

gpui::actions!(tau_view, [Submit, SwitchAgent]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", Submit, None),
        KeyBinding::new("tab", SwitchAgent, None),
    ]);
}

#[derive(Clone, Copy)]
enum Role {
    User,
    Assistant,
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

struct Message {
    role: Role,
    text: String,
}

pub struct TauView {
    input: Entity<TextInput>,
    backend: Backend,
    messages: Vec<Message>,
    session_id: Option<String>,
    assistant_index: Option<usize>,
    usage: Option<String>,
    status: String,
    task: Option<Task<()>>,
    agent: AgentMode,
}

impl TauView {
    pub fn new(backend: Backend, cx: &mut Context<Self>) -> Self {
        let input = cx.new(TextInput::new);
        Self {
            input,
            backend,
            messages: Vec::new(),
            session_id: None,
            assistant_index: None,
            usage: None,
            status: "Ready".into(),
            task: None,
            agent: AgentMode::Code,
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
        self.status = format!("{} agent", self.agent.label());
        cx.notify();
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.assistant_index.is_some() {
            return;
        }
        let prompt = self.input.read(cx).content();
        if prompt.trim().is_empty() {
            return;
        }
        self.input.update(cx, |input, _| input.reset());
        self.messages.push(Message {
            role: Role::User,
            text: prompt.clone(),
        });
        self.messages.push(Message {
            role: Role::Assistant,
            text: String::new(),
        });
        self.assistant_index = Some(self.messages.len() - 1);
        self.status = "Thinking...".into();
        self.usage = None;
        cx.notify();
        window.focus(&self.input.focus_handle(cx));

        let receiver = self.backend.completion(prompt, self.session_id.clone());
        self.task = Some(cx.spawn(async move |this, cx| {
            let mut receiver = receiver;
            while let Some(event) = receiver.recv().await {
                let done = matches!(event, Ok(CompletionEvent::Complete(_)) | Err(_));
                let _ = this.update(cx, |view, cx| {
                    view.apply_event(event, cx);
                });
                if done {
                    break;
                }
            }
        }));
    }

    fn apply_event(&mut self, event: Result<CompletionEvent, String>, cx: &mut Context<Self>) {
        let Some(index) = self.assistant_index else {
            return;
        };
        match event {
            Ok(CompletionEvent::Delta(delta)) => {
                self.messages[index].text.push_str(&delta.text);
                cx.notify();
            }
            Ok(CompletionEvent::Complete(result)) => self.finish(result, cx),
            Err(error) => {
                self.messages[index].text = format!("Error: {error}");
                self.status = "Request failed".into();
                self.assistant_index = None;
                cx.notify();
            }
        }
    }

    fn finish(&mut self, result: CompletionStreamResult, cx: &mut Context<Self>) {
        if let Some(index) = self.assistant_index.take() {
            self.messages[index].text = result.text;
        }
        self.session_id = Some(result.session_id);
        self.usage = Some(format!("{} tokens", result.usage.total_tokens));
        self.status = "Ready".into();
        cx.notify();
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
                self.status
            )));
        let transcript = div()
            .id("transcript")
            .flex_1()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_3()
            .p_5()
            .children(self.messages.iter().map(|message| {
                let (background, label, align_end) = match message.role {
                    Role::User => (0x274d72, "You", true),
                    Role::Assistant => (0x202630, "tau", false),
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
                            .child(message.text.clone()),
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
                self.usage.as_deref().unwrap_or("No usage yet"),
            ))
            .child(sidebar_card(
                "SESSION",
                self.session_id.as_deref().unwrap_or("Not started"),
            ))
            .child(sidebar_card("DIRECTORY", self.backend.cwd()));
        let footer = div().p_4().border_t_1().border_color(rgb(0x2c3340)).child(
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
        );
        div()
            .size_full()
            .bg(rgb(0x11151b))
            .text_color(rgb(0xe8edf5))
            .flex()
            .flex_col()
            .key_context("TauView")
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::switch_agent))
            .child(header)
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
