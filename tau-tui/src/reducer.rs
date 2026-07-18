use crate::state::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use tau_proto::prelude::{IdempotencyKey, RequestAction, TurnStartParams};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone)]
pub enum Action {
    Insert(char),
    Paste(String),
    Backspace,
    Delete,
    MoveLeft {
        select: bool,
    },
    MoveRight {
        select: bool,
    },
    MoveHome {
        select: bool,
    },
    MoveEnd {
        select: bool,
    },
    MoveUp {
        select: bool,
    },
    MoveDown {
        select: bool,
    },
    Newline,
    Submit,
    Open(Picker),
    ClosePicker,
    Query(char),
    QueryBackspace,
    MovePicker(i8),
    Pick,
    ToggleFavorite,
    Permission(PermissionChoice),
    /// Reply to a pending permission request; unlike the legacy action this is
    /// explicit about the protocol reply and is safe to dispatch from replay.
    PermissionReply(PermissionChoice),
    QuestionReply(String),
    DiffReply(bool),
    ExpandTool(usize),
    NextHunk,
    PrevHunk,
    Hunk(bool),
    AcceptFile,
    Undo,
    Redo,
    ToggleAutonomy,
    ToggleFollow,
    ToggleFeedDetails(u64),
    Tier(i8),
    Cancel,
    Replay,
    SwitchServer,
    Reconnect,
    Connected,
    ToggleSessions,
    SessionMove(i8),
    SessionSearch(char),
    SessionSearchBackspace,
    SessionSelect,
    SessionRename(String),
    SessionArchive,
    SessionRestore,
    SessionToggleArchived,
    NewChat,
    OperationsTab(OperationsTab),
    OperationsRefresh,
    OperationsSelect(i8),
    OperationsOpen,
    OperationsStage,
    OperationsUnstage,
    OperationsRevertConfirmed,
    OperationsKeep,
    OperationsAcknowledge,
    OperationsCreateBranch(String),
    OperationsSwitchBranch(String),
}

pub fn key_action(s: &AppState, k: KeyEvent) -> Option<Action> {
    if s.sessions.open {
        if k.code == KeyCode::Char('a') && k.modifiers.contains(KeyModifiers::CONTROL) {
            return Some(Action::SessionToggleArchived);
        }
        return match k.code {
            KeyCode::Esc => Some(Action::ToggleSessions),
            KeyCode::Up => Some(Action::SessionMove(-1)),
            KeyCode::Down => Some(Action::SessionMove(1)),
            KeyCode::Enter => Some(Action::SessionSelect),
            KeyCode::Backspace => Some(Action::SessionSearchBackspace),
            KeyCode::Char(c) => Some(Action::SessionSearch(c)),
            _ => None,
        };
    }
    if s.permission.is_some() {
        return match k.code {
            KeyCode::Left | KeyCode::Char('h') => {
                Some(Action::Permission(PermissionChoice::AllowOnce))
            }
            KeyCode::Right | KeyCode::Char('l') => {
                Some(Action::Permission(PermissionChoice::AllowAlways))
            }
            KeyCode::Char('s') => Some(Action::Permission(PermissionChoice::AllowSession)),
            KeyCode::Char('d') => Some(Action::Permission(PermissionChoice::DenyOnce)),
            KeyCode::Enter => Some(Action::Permission(PermissionChoice::AllowOnce)),
            KeyCode::Backspace => Some(Action::Permission(PermissionChoice::Reject)),
            _ => None,
        };
    }
    if s.question.is_some() {
        if matches!(k.code, KeyCode::Enter) {
            return Some(Action::QuestionReply(s.input.clone()));
        }
        if matches!(k.code, KeyCode::Esc) {
            return Some(Action::QuestionReply(String::new()));
        }
    }
    // Diff requests are protocol-gated just like permissions and questions.
    // Keep this modal ahead of hunk navigation so Enter/Backspace always
    // produces the daemon acknowledgement when no local hunks are present.
    if s.diff_reply.is_some() {
        return match k.code {
            KeyCode::Enter | KeyCode::Char('y') => Some(Action::DiffReply(true)),
            KeyCode::Backspace | KeyCode::Char('n') => Some(Action::DiffReply(false)),
            _ => None,
        };
    }
    if !s.hunks.is_empty() && s.input.is_empty() {
        if k.code == KeyCode::Enter {
            return Some(Action::Hunk(true));
        }
        if k.code == KeyCode::Backspace {
            return Some(Action::Hunk(false));
        }
    }
    if s.picker != Picker::None {
        return match k.code {
            KeyCode::Esc => Some(Action::ClosePicker),
            KeyCode::Enter => Some(Action::Pick),
            KeyCode::Backspace => Some(Action::QueryBackspace),
            KeyCode::Up => Some(Action::MovePicker(-1)),
            KeyCode::Down => Some(Action::MovePicker(1)),
            KeyCode::Char(c) => Some(Action::Query(c)),
            _ => None,
        };
    }
    match k.code {
        KeyCode::Char('s') if k.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Action::ToggleSessions)
        }
        KeyCode::Char('n') if k.modifiers.contains(KeyModifiers::CONTROL) => Some(Action::NewChat),
        KeyCode::F(6) => Some(Action::OperationsTab(OperationsTab::Status)),
        KeyCode::F(7) => Some(Action::OperationsTab(OperationsTab::Git)),
        KeyCode::F(8) => Some(Action::OperationsTab(OperationsTab::Changes)),
        KeyCode::Tab if s.operations_focused => {
            Some(Action::OperationsTab(match s.operations_tab {
                OperationsTab::Status => OperationsTab::Git,
                OperationsTab::Git => OperationsTab::Changes,
                OperationsTab::Changes => OperationsTab::Status,
            }))
        }
        KeyCode::Enter if s.operations_focused => Some(Action::OperationsOpen),
        KeyCode::Char('R') if s.operations_focused => Some(Action::OperationsRefresh),
        KeyCode::Char('s') if s.operations_focused => Some(Action::OperationsStage),
        KeyCode::Char('u') if s.operations_focused => Some(Action::OperationsUnstage),
        KeyCode::Char('K') if s.operations_focused => Some(Action::OperationsKeep),
        // Keep Ctrl-A's existing accept-file binding; plain `a` is the
        // operations-only acknowledgement shortcut.
        KeyCode::Char('a')
            if s.operations_focused && !k.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            Some(Action::OperationsAcknowledge)
        }
        KeyCode::Char('v') if s.operations_focused => Some(Action::OperationsRevertConfirmed),
        KeyCode::Char('c') if s.operations_focused => {
            Some(Action::OperationsCreateBranch("tau/tui-review".into()))
        }
        KeyCode::Char('b') if s.operations_focused => s
            .operations
            .branches
            .iter()
            .find(|branch| !branch.current)
            .map(|branch| Action::OperationsSwitchBranch(branch.name.clone())),
        KeyCode::Char('o') if s.operations_focused => Some(Action::OperationsOpen),
        KeyCode::Up if s.operations_focused => Some(Action::OperationsSelect(-1)),
        KeyCode::Down if s.operations_focused => Some(Action::OperationsSelect(1)),
        KeyCode::Char(c) if k.modifiers.contains(KeyModifiers::CONTROL) && c == 'c' => {
            Some(Action::Cancel)
        }
        KeyCode::Char(']') => Some(Action::NextHunk),
        KeyCode::Char('[') => Some(Action::PrevHunk),
        KeyCode::Char('u') => Some(Action::Undo),
        KeyCode::Char('U') => Some(Action::Redo),
        KeyCode::Left => Some(Action::MoveLeft {
            select: k.modifiers.contains(KeyModifiers::SHIFT),
        }),
        KeyCode::Right => Some(Action::MoveRight {
            select: k.modifiers.contains(KeyModifiers::SHIFT),
        }),
        KeyCode::Up => Some(Action::MoveUp {
            select: k.modifiers.contains(KeyModifiers::SHIFT),
        }),
        KeyCode::Down => Some(Action::MoveDown {
            select: k.modifiers.contains(KeyModifiers::SHIFT),
        }),
        KeyCode::Home => Some(Action::MoveHome {
            select: k.modifiers.contains(KeyModifiers::SHIFT),
        }),
        KeyCode::End => Some(Action::MoveEnd {
            select: k.modifiers.contains(KeyModifiers::SHIFT),
        }),
        KeyCode::Delete => Some(Action::Delete),
        KeyCode::Char('a') if k.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Action::AcceptFile)
        }
        KeyCode::Char('r') if k.modifiers.contains(KeyModifiers::CONTROL) => Some(Action::Replay),
        KeyCode::Char('f') => Some(Action::ToggleFollow),
        KeyCode::Char(c) => Some(Action::Insert(c)),
        KeyCode::Backspace => Some(Action::Backspace),
        KeyCode::Enter if k.modifiers.contains(KeyModifiers::SHIFT) => Some(Action::Newline),
        KeyCode::Enter => Some(Action::Submit),
        KeyCode::Tab => Some(Action::SwitchServer),
        KeyCode::F(2) => Some(Action::ToggleAutonomy),
        KeyCode::F(3) => Some(Action::Tier(1)),
        KeyCode::F(4) => Some(Action::Tier(-1)),
        _ => None,
    }
}

/// Translate terminal mouse input into the same typed actions as keyboard
/// input. Coordinates are deliberately coarse: cards are line-oriented and
/// the renderer keeps them in transcript order.
pub fn mouse_action(s: &AppState, mouse: MouseEvent) -> Option<Action> {
    if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
        return None;
    }
    let row = mouse.row.saturating_sub(1) as usize;
    if !s.raw_events.is_empty() {
        let event_row = row.saturating_sub(s.human_messages.len().saturating_mul(2));
        let event_index = event_row / 2;
        if let Some(event) = s.raw_events.get(event_index) {
            return Some(Action::ToggleFeedDetails(event.sequence));
        }
    }
    if row < s.tools.len() {
        Some(Action::ExpandTool(row))
    } else if !s.hunks.is_empty() {
        Some(Action::Hunk(true))
    } else {
        None
    }
}

pub fn apply(s: &mut AppState, a: Action) -> Option<String> {
    // Composer owns all prompt editing.  The fields used by the renderer and
    // protocol adapter are refreshed only as a typed projection below.
    if let Some(result) = apply_composer_action(s, &a) {
        return result;
    }
    match a {
        Action::Insert(c) => {
            replace_selection(s);
            let value = c.to_string();
            s.input.insert_str(s.cursor, &value);
            s.cursor += value.len();
        }
        Action::Paste(text) => {
            replace_selection(s);
            s.input.insert_str(s.cursor, &text);
            s.cursor += text.len();
        }
        Action::Backspace => {
            if s.selection.is_some() {
                replace_selection(s);
            } else if s.cursor > 0 {
                let p = s.input[..s.cursor]
                    .grapheme_indices(true)
                    .next_back()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                s.input.drain(p..s.cursor);
                s.cursor = p;
            }
        }
        Action::Delete => {
            if s.selection.is_some() {
                replace_selection(s);
            } else if s.cursor < s.input.len() {
                let end = s.input[s.cursor..]
                    .grapheme_indices(true)
                    .nth(1)
                    .map(|(i, _)| s.cursor + i)
                    .unwrap_or(s.input.len());
                s.input.drain(s.cursor..end);
            }
        }
        Action::MoveLeft { select } => move_horizontal(s, -1, select),
        Action::MoveRight { select } => move_horizontal(s, 1, select),
        Action::MoveHome { select } => move_line_edge(s, false, select),
        Action::MoveEnd { select } => move_line_edge(s, true, select),
        Action::MoveUp { select } => move_vertical(s, -1, select),
        Action::MoveDown { select } => move_vertical(s, 1, select),
        Action::Newline => {
            s.input.insert(s.cursor, '\n');
            s.cursor += 1;
        }
        Action::Submit => {
            if !s.input.trim().is_empty() {
                let p = std::mem::take(&mut s.input);
                s.cursor = 0;
                s.human_messages.push(p.clone());
                s.transcript.push(format!("You: {p}"));
                if let Some(agent) = p.strip_prefix("/agent ") {
                    s.agent = agent.trim().to_string();
                    s.picker = Picker::None;
                } else if let Some(model) = p.strip_prefix("/model ") {
                    s.model = model.trim().to_string();
                    s.picker = Picker::None;
                } else if p.trim() == "/agents" || p.trim() == "/agent" {
                    s.picker = Picker::Agents;
                } else if p.trim() == "/models" || p.trim() == "/model" {
                    s.picker = Picker::Models;
                }
                sync_composer(s);
                return Some(p);
            }
        }
        Action::Open(p) => {
            s.picker = p;
            s.picker_query.clear();
            s.picker_index = 0;
        }
        Action::ClosePicker => s.picker = Picker::None,
        Action::Query(c) => s.picker_query.push(c),
        Action::QueryBackspace => {
            s.picker_query.pop();
            s.picker_index = 0;
        }
        Action::MovePicker(delta) => {
            let count = if s.picker == Picker::Models {
                filtered_models(s).len()
            } else {
                s.agents.len()
            };
            if count > 0 {
                s.picker_index = if delta < 0 {
                    s.picker_index.saturating_sub(1)
                } else {
                    (s.picker_index + 1).min(count - 1)
                };
            }
        }
        Action::Pick => {
            if s.picker == Picker::Models {
                if let Some(m) = filtered_models(s).get(s.picker_index) {
                    s.model = m.id.clone();
                    s.recent_models.retain(|id| id != &s.model);
                    s.recent_models.insert(0, s.model.clone());
                    s.recent_models.truncate(8);
                }
            } else if s.picker == Picker::Agents {
                if let Some(a) = s
                    .agents
                    .iter()
                    .find(|a| a.to_lowercase().contains(&s.picker_query.to_lowercase()))
                {
                    s.agent = a.clone();
                }
            } else if s.picker == Picker::Commands && !s.picker_query.trim().is_empty() {
                s.input = s.picker_query.clone();
                s.cursor = s.input.len();
            }
            s.picker = Picker::None;
        }
        Action::ToggleFavorite => {
            if let Some(m) = filtered_models(s).get(s.picker_index) {
                let id = m.id.clone();
                if let Some(model) = s.models.iter_mut().find(|model| model.id == id) {
                    model.favorite = !model.favorite;
                }
            }
        }
        Action::Permission(c) => {
            if let Some(p) = s.permission.as_mut() {
                if p.stage == PermissionStage::AlwaysConfirm && c == PermissionChoice::AllowOnce {
                    s.permission = None;
                    s.permission_request_id = None;
                    return None;
                }
                p.choice = c;
                if c == PermissionChoice::AllowAlways || c == PermissionChoice::AllowSession {
                    p.stage = PermissionStage::AlwaysConfirm;
                } else {
                    s.permission = None;
                    s.permission_request_id = None;
                }
            }
        }
        Action::PermissionReply(c) => {
            if let Some(p) = s.permission.as_mut() {
                p.choice = c;
                if c == PermissionChoice::AllowAlways || c == PermissionChoice::AllowSession {
                    p.stage = PermissionStage::AlwaysConfirm;
                } else {
                    s.permission = None;
                    s.permission_request_id = None;
                }
                return Some(format!("permission:{c:?}"));
            }
        }
        Action::QuestionReply(answer) => {
            if let Some(q) = s.question.as_mut() {
                q.answer = Some(answer.clone());
                s.question = None;
                s.question_id = None;
                s.input.clear();
                s.cursor = 0;
                let _ = s.composer.apply(crate::composer::ComposerAction::Clear);
                sync_composer(s);
                return Some(answer);
            }
        }
        Action::DiffReply(accepted) => {
            if let Some(reply) = s.diff_reply.as_mut() {
                reply.accepted = Some(accepted);
                s.diff_request_id = None;
                s.diff_path = None;
                return Some(if accepted {
                    "diff:accept".into()
                } else {
                    "diff:reject".into()
                });
            }
            if s.hunk_index < s.hunks.len() {
                s.hunks[s.hunk_index].accepted = Some(accepted);
            }
        }
        Action::ExpandTool(i) => {
            if let Some(t) = s.tools.get_mut(i) {
                t.expanded = !t.expanded;
            }
        }
        Action::NextHunk => {
            if !s.hunks.is_empty() {
                s.hunk_index = (s.hunk_index + 1) % s.hunks.len();
            }
        }
        Action::PrevHunk => {
            if !s.hunks.is_empty() {
                s.hunk_index = (s.hunk_index + s.hunks.len() - 1) % s.hunks.len();
            }
        }
        Action::Hunk(v) => {
            if s.hunk_index < s.hunks.len() {
                let old = s.hunks.iter().map(|h| h.accepted).collect();
                s.undo.push_back(old);
                let h = &mut s.hunks[s.hunk_index];
                h.accepted = Some(v);
                s.redo.clear();
            }
        }
        Action::AcceptFile => {
            if !s.hunks.is_empty() {
                s.undo
                    .push_back(s.hunks.iter().map(|h| h.accepted).collect());
                for h in &mut s.hunks {
                    h.accepted = Some(true);
                }
                s.redo.clear();
            }
        }
        Action::Undo => {
            if let Some(v) = s.undo.pop_back() {
                s.redo
                    .push_back(s.hunks.iter().map(|h| h.accepted).collect());
                for (h, x) in s.hunks.iter_mut().zip(v) {
                    h.accepted = x;
                }
            }
        }
        Action::Redo => {
            if let Some(v) = s.redo.pop_back() {
                s.undo
                    .push_back(s.hunks.iter().map(|h| h.accepted).collect());
                for (h, x) in s.hunks.iter_mut().zip(v) {
                    h.accepted = x;
                }
            }
        }
        Action::ToggleAutonomy => s.autonomous = !s.autonomous,
        Action::ToggleFollow => s.following = !s.following,
        Action::ToggleFeedDetails(sequence) => {
            if !s.expanded_feed.remove(&sequence) {
                s.expanded_feed.insert(sequence);
            }
        }
        Action::Tier(d) => s.task_tier = (s.task_tier as i8 + d).clamp(1, 3) as u8,
        Action::Cancel => {
            s.cancelling = true;
            s.replaying = false;
        }
        Action::Replay => {
            s.replaying = true;
            s.cancelling = false;
            s.connection = Connection::Reconnecting;
        }
        Action::SwitchServer => {
            if !s.servers.is_empty() {
                s.server_index = (s.server_index + 1) % s.servers.len();
            }
        }
        Action::Reconnect => s.connection = Connection::Reconnecting,
        Action::Connected => {
            s.connection = Connection::Connected;
            s.replaying = false;
        }
        Action::ToggleSessions => {
            s.sessions.open = !s.sessions.open;
            s.sessions.selected = 0;
        }
        Action::SessionMove(delta) => s.sessions.select_delta(delta),
        Action::SessionSearch(c) => {
            s.sessions.query.push(c);
            s.sessions.selected = 0;
        }
        Action::SessionSearchBackspace => {
            s.sessions.query.pop();
            s.sessions.selected = 0;
        }
        Action::SessionSelect => {
            if let Some(id) = s.sessions.selected_id() {
                s.session_id = Some(id.as_str().to_owned());
                s.sessions.open = false;
                s.replaying = true;
                s.connection = Connection::Reconnecting;
                return Some("/replay".into());
            }
        }
        Action::SessionRename(title) => {
            if let Some(id) = s.sessions.selected_id() {
                s.sessions.rename(&id, title);
            }
        }
        Action::SessionArchive => {
            if let Some(id) = s.sessions.selected_id() {
                s.sessions.archive(&id);
            }
        }
        Action::SessionRestore => {
            if let Some(id) = s.sessions.selected_id() {
                s.sessions.restore(&id);
            }
        }
        Action::SessionToggleArchived => s.sessions.toggle_archived(),
        Action::NewChat => {
            s.session_id = None;
            s.turn_id = None;
            s.sequence = 0;
            s.raw_events.clear();
            s.transcript = vec!["New chat".into()];
            s.sessions.open = false;
        }
        Action::OperationsTab(tab) => {
            s.operations_tab = tab;
            s.operations_focused = true;
        }
        Action::OperationsSelect(delta) => {
            let n = s.operations.files.len();
            if n > 0 {
                s.operations.selected = if delta < 0 {
                    s.operations.selected.saturating_sub(1)
                } else {
                    (s.operations.selected + 1).min(n - 1)
                };
            }
        }
        Action::OperationsRefresh => {
            s.operations_loading = true;
            s.operations_error = None;
        }
        Action::OperationsOpen => {
            if let Some(path) = s.operations.path().map(str::to_owned) {
                crate::operations::reduce(&mut s.operations, crate::operations::Action::Open(path));
                s.operations_loading = true;
            }
        }
        Action::OperationsKeep => {
            if let Some(path) = s.operations.path().map(str::to_owned) {
                crate::operations::reduce(&mut s.operations, crate::operations::Action::Keep(path));
            }
        }
        Action::OperationsStage
        | Action::OperationsUnstage
        | Action::OperationsRevertConfirmed
        | Action::OperationsAcknowledge
        | Action::OperationsCreateBranch(_)
        | Action::OperationsSwitchBranch(_) => s.operations_loading = true,
    }
    sync_composer(s);
    None
}

pub fn sync_composer(s: &mut AppState) {
    s.input = s.composer.text().to_owned();
    let selection = s.composer.selection();
    s.cursor = selection.cursor;
    s.selection = (selection.anchor != selection.cursor).then_some(selection.anchor);
    s.clipboard = s.composer.clipboard().to_owned();
    s.model = s.composer.state().model.clone().unwrap_or_default();
    s.agent = s.composer.state().agent.clone().unwrap_or_default();
    s.picker = match s.composer.state().picker {
        Some(crate::composer::PickerKind::Model) => Picker::Models,
        Some(crate::composer::PickerKind::Agent) => Picker::Agents,
        Some(crate::composer::PickerKind::Command) => Picker::Commands,
        None => Picker::None,
    };
    s.picker_query = s.composer.state().slash_query.clone().unwrap_or_default();
    s.picker_index = s.composer.state().picker_index;
    let _ = s
        .composer
        .apply(crate::composer::ComposerAction::SetSending(
            s.turn_id.is_some(),
        ));
    let _ = s
        .composer
        .apply(crate::composer::ComposerAction::SetConnection {
            connected: s.connection == Connection::Connected,
            detail: format!("{:?}", s.connection),
        });
}

fn apply_composer_action(s: &mut AppState, action: &Action) -> Option<Option<String>> {
    use crate::composer::ComposerAction as C;
    let mapped = match action {
        Action::Insert(c) => C::Insert(c.to_string()),
        Action::Paste(v) => C::Paste(v.clone()),
        Action::Backspace => C::Backspace,
        Action::Delete => C::Delete,
        Action::MoveLeft { select } => C::MoveLeft { selecting: *select },
        Action::MoveRight { select } => C::MoveRight { selecting: *select },
        Action::MoveUp { select } => C::MoveUp { selecting: *select },
        Action::MoveDown { select } => C::MoveDown { selecting: *select },
        Action::MoveHome { select } => C::MoveHome { selecting: *select },
        Action::MoveEnd { select } => C::MoveEnd { selecting: *select },
        Action::Newline => C::Newline,
        Action::Undo => C::Undo,
        Action::Redo => C::Redo,
        Action::Cancel => {
            s.cancelling = true;
            s.replaying = false;
            C::Cancel
        }
        Action::Open(Picker::Models) => C::OpenPicker(crate::composer::PickerKind::Model),
        Action::Open(Picker::Agents) => C::OpenPicker(crate::composer::PickerKind::Agent),
        Action::Open(Picker::Commands) => C::OpenPicker(crate::composer::PickerKind::Command),
        Action::ClosePicker => C::ClosePicker,
        Action::Query(c) => C::SlashQuery(format!("{}{}", s.picker_query, c)),
        Action::QueryBackspace => C::SlashQuery(
            s.picker_query
                .graphemes(true)
                .next_back()
                .map_or_else(String::new, |last| {
                    s.picker_query[..s.picker_query.len() - last.len()].to_owned()
                }),
        ),
        Action::MovePicker(delta) => {
            let count = match s.composer.state().picker {
                Some(crate::composer::PickerKind::Model) => filtered_models(s).len(),
                Some(crate::composer::PickerKind::Agent) => s.agents.len(),
                Some(crate::composer::PickerKind::Command) => command_count(s),
                None => 0,
            };
            let index = if count == 0 {
                0
            } else {
                (s.composer.state().picker_index as isize + *delta as isize)
                    .rem_euclid(count as isize) as usize
            };
            C::SetPickerIndex(index)
        }
        Action::Pick => match s.composer.state().picker {
            Some(crate::composer::PickerKind::Model) => filtered_models(s)
                .get(s.composer.state().picker_index)
                .map(|model| C::ChooseModel(model.id.clone()))
                .unwrap_or(C::ClosePicker),
            Some(crate::composer::PickerKind::Agent) => s
                .agents
                .iter()
                .filter(|agent| {
                    s.picker_query.is_empty()
                        || agent
                            .to_lowercase()
                            .contains(&s.picker_query.to_lowercase())
                })
                .nth(s.composer.state().picker_index)
                .map(|agent| C::ChooseAgent(agent.clone()))
                .unwrap_or(C::ClosePicker),
            Some(crate::composer::PickerKind::Command) => {
                C::ChooseSlashCommand(s.picker_query.clone())
            }
            None => return None,
        },
        Action::Submit => {
            if !s.composer.send_enabled() {
                return Some(None);
            }
            let prompt = s.composer.text().to_owned();
            s.composer.record_history(prompt.clone());
            let _ = s.composer.apply(C::Send);
            let _ = s.composer.apply(C::Clear);
            let _ = s.composer.apply(C::SetSending(false));
            sync_composer(s);
            s.transcript.push(format!("You: {prompt}"));
            return Some(Some(prompt));
        }
        _ => return None,
    };
    s.composer.apply(mapped);
    sync_composer(s);
    Some(None)
}
fn command_count(s: &AppState) -> usize {
    ["/agent", "/agents", "/model", "/help", "/replay"]
        .iter()
        .filter(|command| s.picker_query.is_empty() || command.contains(&s.picker_query))
        .count()
}
pub fn filtered_models(s: &AppState) -> Vec<&Model> {
    let mut models: Vec<&Model> = s
        .models
        .iter()
        .filter(|m| {
            s.picker_query.is_empty()
                || m.id.to_lowercase().contains(&s.picker_query.to_lowercase())
                || m.provider
                    .to_lowercase()
                    .contains(&s.picker_query.to_lowercase())
        })
        .collect();
    models.sort_by_key(|m| (!m.favorite, !s.recent_models.contains(&m.id)));
    models
}
pub fn params(
    s: &AppState,
    prompt: String,
    cwd: Option<String>,
) -> anyhow::Result<TurnStartParams> {
    let Some(project_id) = s
        .project_id
        .clone()
        .filter(|project_id| !project_id.trim().is_empty())
    else {
        anyhow::bail!("cannot start turn: select an active project first");
    };
    let idempotency_key = IdempotencyKey::new(format!("tau-tui-{}", uuid_like(prompt.as_bytes())));
    let action = if prompt.starts_with('/') {
        RequestAction::Command {
            command: prompt.clone(),
        }
    } else {
        RequestAction::Submit
    };
    Ok(TurnStartParams {
        project_id,
        model: s.model.clone(),
        prompt,
        session_id: s.session_id.clone(),
        cwd,
        idempotency_key,
        agent: Some(s.agent.clone()),
        task_tier: Some(s.task_tier),
        autonomous: Some(s.autonomous),
        action: Some(action),
    })
}

fn uuid_like(bytes: &[u8]) -> String {
    let mut value = 1469598103934665603u64;
    for byte in bytes {
        value = (value ^ u64::from(*byte)).wrapping_mul(1099511628211);
    }
    format!("{value:016x}")
}

fn replace_selection(s: &mut AppState) {
    if let Some(anchor) = s.selection.take() {
        let (start, end) = if anchor <= s.cursor {
            (anchor, s.cursor)
        } else {
            (s.cursor, anchor)
        };
        s.input.drain(start..end);
        s.cursor = start;
    }
}

fn move_horizontal(s: &mut AppState, direction: i8, select: bool) {
    let next = if direction < 0 {
        s.input[..s.cursor]
            .char_indices()
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0)
    } else {
        s.cursor
            + s.input[s.cursor..]
                .grapheme_indices(true)
                .nth(1)
                .map(|(i, _)| i)
                .unwrap_or(0)
    };
    if select {
        if s.selection.is_none() {
            s.selection = Some(s.cursor);
        }
    } else {
        s.selection = None;
    }
    s.cursor = next;
}

fn move_line_edge(s: &mut AppState, end: bool, select: bool) {
    let line_start = s.input[..s.cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = s.input[s.cursor..]
        .find('\n')
        .map(|i| s.cursor + i)
        .unwrap_or(s.input.len());
    if select {
        if s.selection.is_none() {
            s.selection = Some(s.cursor);
        }
    } else {
        s.selection = None;
    }
    s.cursor = if end { line_end } else { line_start };
}

fn move_vertical(s: &mut AppState, direction: i8, select: bool) {
    let start = s.input[..s.cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let column = s.cursor - start;
    let target = if direction < 0 {
        if start == 0 {
            0
        } else {
            let prev_end = start - 1;
            let prev_start = s.input[..prev_end].rfind('\n').map(|i| i + 1).unwrap_or(0);
            prev_start + column.min(prev_end - prev_start)
        }
    } else if let Some(next_rel) = s.input[s.cursor..].find('\n') {
        let next_start = s.cursor + next_rel + 1;
        let next_end = s.input[next_start..]
            .find('\n')
            .map(|i| next_start + i)
            .unwrap_or(s.input.len());
        next_start + column.min(next_end - next_start)
    } else {
        s.input.len()
    };
    if select {
        if s.selection.is_none() {
            s.selection = Some(s.cursor);
        }
    } else {
        s.selection = None;
    }
    s.cursor = target;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiline_editor_moves_and_restores_snapshot() {
        let mut state = AppState::default();
        apply(&mut state, Action::Paste("one\ntwo".into()));
        apply(&mut state, Action::MoveUp { select: false });
        assert!(state.cursor < 4);
        let snapshot = state.buffer_snapshot();
        apply(&mut state, Action::Insert('!'));
        state.restore_buffer(snapshot);
        assert_eq!(state.input, "one\ntwo");
    }

    #[test]
    fn selection_replaces_only_selected_text() {
        let mut state = AppState::default();
        apply(&mut state, Action::Paste("hello".into()));
        apply(&mut state, Action::MoveHome { select: false });
        apply(&mut state, Action::MoveEnd { select: true });
        apply(&mut state, Action::Paste("H".into()));
        assert_eq!(state.input, "H");
    }

    #[test]
    fn explicit_replies_are_reducer_state_transitions() {
        let mut state = AppState {
            permission: Some(Permission {
                tool: "shell".into(),
                summary: "run".into(),
                choice: PermissionChoice::AllowOnce,
                stage: PermissionStage::Choose,
            }),
            question: Some(Question {
                prompt: "?".into(),
                answer: None,
            }),
            ..AppState::default()
        };
        apply(
            &mut state,
            Action::PermissionReply(PermissionChoice::DenyOnce),
        );
        assert!(state.permission.is_none());
        assert_eq!(
            apply(&mut state, Action::QuestionReply("yes".into())),
            Some("yes".into())
        );
        assert!(state.question.is_none());
    }

    #[test]
    fn replay_cancel_and_connection_transitions_are_exclusive() {
        let mut state = AppState::default();
        apply(&mut state, Action::Replay);
        assert!(state.replaying);
        assert_eq!(state.connection, Connection::Reconnecting);
        apply(&mut state, Action::Cancel);
        assert!(state.cancelling);
        assert!(!state.replaying);
        apply(&mut state, Action::Connected);
        assert_eq!(state.connection, Connection::Connected);
    }

    #[test]
    fn navigator_keyboard_actions_drive_search_selection_and_archive_flow() {
        use crate::sessions::{ProjectId, SessionEntry, SessionId};
        use crossterm::event::KeyEvent;

        let mut state = AppState::default();
        state.sessions.project = Some(ProjectId::new("p"));
        state.sessions.sessions = vec![
            SessionEntry {
                id: SessionId::new("old"),
                project_id: ProjectId::new("p"),
                title: "Old".into(),
                updated_at: 1,
                archived: false,
            },
            SessionEntry {
                id: SessionId::new("new"),
                project_id: ProjectId::new("p"),
                title: "Newest".into(),
                updated_at: 2,
                archived: false,
            },
        ];
        apply(&mut state, Action::ToggleSessions);
        let down = key_action(&state, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)).unwrap();
        apply(&mut state, down);
        assert_eq!(state.sessions.selected_id().unwrap().as_str(), "old");
        apply(&mut state, Action::SessionSearch('o'));
        assert_eq!(state.sessions.selected_id().unwrap().as_str(), "old");
        apply(&mut state, Action::SessionArchive);
        assert!(state.sessions.visible().is_empty());
        apply(&mut state, Action::SessionToggleArchived);
        apply(&mut state, Action::SessionRestore);
        assert!(!state.sessions.sessions[0].archived);
        let esc = key_action(&state, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).unwrap();
        assert!(matches!(esc, Action::ToggleSessions));
    }

    #[test]
    fn diff_request_keys_are_explicit_ack_actions() {
        let state = AppState {
            diff_reply: Some(DiffReply { accepted: None }),
            ..AppState::default()
        };
        assert!(matches!(
            key_action(&state, KeyEvent::from(KeyCode::Char('y'))),
            Some(Action::DiffReply(true))
        ));
        assert!(matches!(
            key_action(&state, KeyEvent::from(KeyCode::Char('n'))),
            Some(Action::DiffReply(false))
        ));
    }

    #[test]
    fn feed_detail_toggle_is_stateful_and_typed_by_sequence() {
        let mut state = AppState::default();
        apply(&mut state, Action::ToggleFeedDetails(7));
        assert!(state.expanded_feed.contains(&7));
        apply(&mut state, Action::ToggleFeedDetails(7));
        assert!(!state.expanded_feed.contains(&7));
    }
}
