use crate::state::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use tau_proto::prelude::{IdempotencyKey, RequestAction, TurnStartParams};

#[derive(Debug, Clone)]
pub enum Action {
    Insert(char),
    Paste(String),
    Backspace,
    Delete,
    MoveLeft { select: bool },
    MoveRight { select: bool },
    MoveHome { select: bool },
    MoveEnd { select: bool },
    MoveUp { select: bool },
    MoveDown { select: bool },
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
    ExpandTool(usize),
    NextHunk,
    PrevHunk,
    Hunk(bool),
    AcceptFile,
    Undo,
    Redo,
    ToggleAutonomy,
    Tier(i8),
    Cancel,
    Replay,
    SwitchServer,
    Reconnect,
}

pub fn key_action(s: &AppState, k: KeyEvent) -> Option<Action> {
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
    if row < s.tools.len() {
        Some(Action::ExpandTool(row))
    } else if !s.hunks.is_empty() {
        Some(Action::Hunk(true))
    } else {
        None
    }
}

pub fn apply(s: &mut AppState, a: Action) -> Option<String> {
    match a {
        Action::Insert(c) => {
            replace_selection(s);
            s.input.insert(s.cursor, c);
            s.cursor += c.len_utf8();
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
                    .char_indices()
                    .last()
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
                let n = s.input[s.cursor..]
                    .char_indices()
                    .nth(1)
                    .map(|(i, _)| i)
                    .unwrap_or(s.input.len() - s.cursor);
                s.input.drain(s.cursor..s.cursor + n);
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
                s.transcript.push(format!("You: {p}"));
                if let Some(agent) = p.strip_prefix("/agent ") {
                    s.agent = agent.trim().to_string();
                    s.picker = Picker::None;
                } else if p.trim() == "/agents" || p.trim() == "/agent" {
                    s.picker = Picker::Agents;
                }
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
                    return None;
                }
                p.choice = c;
                if c == PermissionChoice::AllowAlways || c == PermissionChoice::AllowSession {
                    p.stage = PermissionStage::AlwaysConfirm;
                } else {
                    s.permission = None;
                }
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
        Action::Tier(d) => s.task_tier = (s.task_tier as i8 + d).clamp(1, 3) as u8,
        Action::Cancel => s.cancelling = true,
        Action::Replay => s.replaying = true,
        Action::SwitchServer => {
            if !s.servers.is_empty() {
                s.server_index = (s.server_index + 1) % s.servers.len();
            }
        }
        Action::Reconnect => s.connection = Connection::Reconnecting,
    }
    None
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
pub fn params(s: &AppState, prompt: String, cwd: Option<String>) -> TurnStartParams {
    let idempotency_key = IdempotencyKey::new(format!("tau-tui-{}", uuid_like(prompt.as_bytes())));
    let action = if prompt.starts_with('/') {
        RequestAction::Command {
            command: prompt.clone(),
        }
    } else {
        RequestAction::Submit
    };
    TurnStartParams {
        model: s.model.clone(),
        prompt,
        session_id: s.session_id.clone(),
        cwd,
        idempotency_key,
        agent: Some(s.agent.clone()),
        task_tier: Some(s.task_tier),
        autonomous: Some(s.autonomous),
        action: Some(action),
    }
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
                .chars()
                .next()
                .map(char::len_utf8)
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
}
