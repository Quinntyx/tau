use gpui::{
    App, Context, Entity, Focusable, KeyBinding, MouseButton, MouseUpEvent, Render, Task, Window,
    div, prelude::*, px, rgb,
};
use tau_client::TurnStreamEvent;

use crate::backend::{Backend, DaemonAction};
use crate::chat::{Card, ChatAction, ChatState, ChatStatus, PermissionChoice, Role};
use crate::input::{TextInput, command_mode};
use crate::picker::{
    AgentOption, ModelOption, PickerAction, command_action, command_suggestions, model_groups,
    next_agent, parse_command,
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
        PickItem
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", Submit, None),
        KeyBinding::new("tab", SwitchAgent, None),
        KeyBinding::new("escape", DismissPicker, None),
        KeyBinding::new("up", PickerUp, None),
        KeyBinding::new("down", PickerDown, None),
        KeyBinding::new("enter", PickItem, Some("Picker".into())),
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
}

pub struct TauView {
    input: Entity<TextInput>,
    picker_input: Entity<TextInput>,
    backend: Backend,
    chat: ChatState,
    task: Option<Task<()>>,
    agent: AgentMode,
    toast_visible: bool,
    model_index: usize,
    models: Vec<String>,
    sidebar_visible: bool,
    picker: Option<PickerKind>,
    picker_index: usize,
    agents: Vec<AgentOption>,
    preferences: crate::preferences::GuiPreferences,
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
        let toast_visible = backend.auto_started();
        let preferences = crate::preferences::path()
            .ok()
            .and_then(|path| crate::preferences::GuiPreferences::load_from(&path).ok())
            .unwrap_or_default();
        let models: Vec<String> = preferences
            .favorites
            .iter()
            .chain(preferences.recent_models.iter())
            .cloned()
            .collect();
        let selected_model = preferences.selected_model.clone();
        let selected_agent = preferences.selected_agent.clone();
        let default_model = backend.model().to_owned();
        let models = models;
        Self {
            input,
            picker_input,
            backend,
            chat: ChatState::default(),
            task: None,
            agent: if selected_agent.as_deref() == Some("Plan") {
                AgentMode::Plan
            } else {
                AgentMode::Code
            },
            toast_visible,
            model_index: 0,
            models: if models.is_empty() {
                vec![selected_model.unwrap_or(default_model)]
            } else {
                models
            },
            sidebar_visible: preferences.sidebar,
            picker: None,
            picker_index: 0,
            agents: vec![
                AgentOption {
                    name: "Plan".into(),
                    in_tab_cycle: true,
                },
                AgentOption {
                    name: "Code".into(),
                    in_tab_cycle: true,
                },
            ],
            preferences,
        }
    }

    fn submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        self.send(window, cx);
    }

    fn click_send(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.send(window, cx);
    }

    fn switch_agent(&mut self, _: &SwitchAgent, _: &mut Window, cx: &mut Context<Self>) {
        let current = self.agent.label();
        if let Some(next) = next_agent(&self.agents, Some(current), false) {
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
            .map(|id| ModelOption {
                id: id.clone(),
                provider: id.split('/').next().unwrap_or("default").to_owned(),
                recent: self.preferences.recent_models.contains(id),
                favorite: self.preferences.favorites.contains(id),
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
                    PickerAction::Dismiss => self.picker = None,
                }
                self.input.update(cx, |input, _| input.reset());
                cx.notify();
                return;
            }
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
        self.agent = if agent.eq_ignore_ascii_case("plan") {
            AgentMode::Plan
        } else {
            AgentMode::Code
        };
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
                            div()
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
                                .child(row)
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
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x8c96a8))
                    .cursor_pointer()
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::click_model))
                    .child(format!(
                        "{}  ·  {}",
                        current_model,
                        match &self.chat.status {
                            ChatStatus::Ready => "Ready",
                            ChatStatus::Streaming => "Thinking...",
                            ChatStatus::Failed(_) => "Request failed",
                        }
                    )),
            );
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
            }));
        let sidebar = div()
            .w(px(260.))
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .border_l_1()
            .border_color(rgb(0x2c3340))
            .child(
                div()
                    .cursor_pointer()
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::click_agent))
                    .child(sidebar_card("AGENT", self.agent.label())),
            )
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
        root = root.on_action(cx.listener(Self::dismiss_picker));
        root = root.on_action(cx.listener(Self::picker_up));
        root = root.on_action(cx.listener(Self::picker_down));
        root = root.on_action(cx.listener(Self::pick_item));
        if self.toast_visible {
            root = root.child(toast);
        }
        let command_active = self.picker.is_none() && command_mode(&self.input.read(cx).content());
        if let Some(kind) = self.picker {
            root = root.child(self.picker_overlay(cx, kind));
        } else if command_active {
            root = root.child(self.picker_overlay(cx, PickerKind::Command));
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
