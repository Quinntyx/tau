use crate::state::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tau_proto::prelude::CompletionStreamParams;

#[derive(Debug, Clone)]
pub enum Action {
    Insert(char),
    Backspace,
    Newline,
    Submit,
    Open(Picker),
    ClosePicker,
    Query(char),
    Pick,
    Permission(PermissionChoice),
    ExpandTool(usize),
    NextHunk,
    PrevHunk,
    Hunk(bool),
    Undo,
    Redo,
    ToggleAutonomy,
    Tier(i8),
    Cancel,
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
            KeyCode::Enter => Some(Action::Permission(PermissionChoice::AllowOnce)),
            KeyCode::Backspace => Some(Action::Permission(PermissionChoice::Reject)),
            _ => None,
        };
    }
    if s.picker != Picker::None {
        return match k.code {
            KeyCode::Esc => Some(Action::ClosePicker),
            KeyCode::Enter => Some(Action::Pick),
            KeyCode::Backspace => Some(Action::Backspace),
            KeyCode::Up => Some(Action::PrevHunk),
            KeyCode::Down => Some(Action::NextHunk),
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
        KeyCode::Char(c) => Some(Action::Insert(c)),
        KeyCode::Backspace => Some(Action::Backspace),
        KeyCode::Enter if k.modifiers.contains(KeyModifiers::SHIFT) => Some(Action::Newline),
        KeyCode::Enter => Some(Action::Submit),
        KeyCode::Tab => Some(Action::Open(Picker::Models)),
        KeyCode::F(2) => Some(Action::ToggleAutonomy),
        KeyCode::F(3) => Some(Action::Tier(1)),
        KeyCode::F(4) => Some(Action::Tier(-1)),
        _ => None,
    }
}

pub fn apply(s: &mut AppState, a: Action) -> Option<String> {
    match a {
        Action::Insert(c) => {
            s.input.insert(s.cursor, c);
            s.cursor += c.len_utf8();
        }
        Action::Backspace => {
            if s.cursor > 0 {
                let p = s.input[..s.cursor]
                    .char_indices()
                    .last()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                s.input.drain(p..s.cursor);
                s.cursor = p;
            }
        }
        Action::Newline => {
            s.input.insert(s.cursor, '\n');
            s.cursor += 1;
        }
        Action::Submit => {
            if !s.input.trim().is_empty() {
                let p = std::mem::take(&mut s.input);
                s.cursor = 0;
                s.transcript.push(format!("You: {p}"));
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
        Action::Pick => {
            if s.picker == Picker::Models {
                if let Some(m) = filtered_models(s).get(s.picker_index) {
                    s.model = m.id.clone();
                }
            } else if s.picker == Picker::Agents {
                if let Some(a) = s.agents.iter().find(|a| a.contains(&s.picker_query)) {
                    s.agent = a.clone();
                }
            }
            s.picker = Picker::None;
        }
        Action::Permission(c) => {
            if let Some(p) = s.permission.as_mut() {
                p.choice = c;
                if c == PermissionChoice::AllowAlways {
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
        Action::Reconnect => s.connection = Connection::Reconnecting,
    }
    None
}
pub fn filtered_models(s: &AppState) -> Vec<&Model> {
    s.models
        .iter()
        .filter(|m| {
            s.picker_query.is_empty()
                || m.id.to_lowercase().contains(&s.picker_query.to_lowercase())
                || m.provider
                    .to_lowercase()
                    .contains(&s.picker_query.to_lowercase())
        })
        .collect()
}
pub fn params(s: &AppState, prompt: String, cwd: Option<String>) -> CompletionStreamParams {
    CompletionStreamParams {
        model: s.model.clone(),
        prompt,
        session_id: s.session_id.clone(),
        cwd,
    }
}
