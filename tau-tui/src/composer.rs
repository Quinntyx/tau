//! Pure prompt composer state.  This module deliberately contains no terminal,
//! protocol, or rendering concerns; callers can translate its actions in their
//! own reducer and submit the resulting request with the stored IDs.

use std::path::Path;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub cursor: usize,
}

impl Selection {
    pub fn caret(at: usize) -> Self {
        Self {
            anchor: at,
            cursor: at,
        }
    }

    pub fn range(self) -> (usize, usize) {
        (self.anchor.min(self.cursor), self.anchor.max(self.cursor))
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionStatus {
    pub connected: bool,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerState {
    pub text: String,
    pub selection: Selection,
    pub model: Option<String>,
    pub agent: Option<String>,
    pub slash_query: Option<String>,
    pub picker: Option<PickerKind>,
    pub character_count: usize,
    pub disabled: bool,
    pub sending: bool,
    pub connection: ConnectionStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerKind {
    Model,
    Agent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerAction {
    Insert(String),
    Paste(String),
    Newline,
    Backspace,
    Delete,
    MoveLeft { selecting: bool },
    MoveRight { selecting: bool },
    MoveHome { selecting: bool },
    MoveEnd { selecting: bool },
    SelectAll,
    Copy,
    Cut,
    Undo,
    Redo,
    Clear,
    InsertFileReference(String),
    SlashQuery(String),
    Autocomplete(String),
    OpenPicker(PickerKind),
    ChooseModel(String),
    ChooseAgent(String),
    ClosePicker,
    SetDisabled(bool),
    Send,
    Cancel,
    SetSending(bool),
    SetConnection { connected: bool, detail: String },
    HistoryPrevious,
    HistoryNext,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    text: String,
    selection: Selection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Composer {
    session_id: String,
    project_id: String,
    state: ComposerState,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    clipboard: String,
    history: Vec<String>,
    history_index: Option<usize>,
}

impl Composer {
    pub fn new(session_id: impl Into<String>, project_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            project_id: project_id.into(),
            state: ComposerState {
                text: String::new(),
                selection: Selection::default(),
                model: None,
                agent: None,
                slash_query: None,
                picker: None,
                character_count: 0,
                disabled: false,
                sending: false,
                connection: ConnectionStatus {
                    connected: false,
                    detail: String::new(),
                },
            },
            undo: Vec::new(),
            redo: Vec::new(),
            clipboard: String::new(),
            history: Vec::new(),
            history_index: None,
        }
    }
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn project_id(&self) -> &str {
        &self.project_id
    }
    pub fn state(&self) -> &ComposerState {
        &self.state
    }
    pub fn text(&self) -> &str {
        &self.state.text
    }
    pub fn selection(&self) -> Selection {
        self.state.selection
    }

    /// Replace the reducer projection without recording an additional undo
    /// step. Reducers use this when protocol events update the legacy fields.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.state.text = text.into();
        let end = self.state.text.len();
        self.state.selection = Selection::caret(end);
        self.state.character_count = self.state.text.graphemes(true).count();
    }

    pub fn set_selection(&mut self, anchor: usize, cursor: usize) {
        self.state.selection = Selection {
            anchor: anchor.min(self.state.text.len()),
            cursor: cursor.min(self.state.text.len()),
        };
    }
    pub fn clipboard(&self) -> &str {
        &self.clipboard
    }
    pub fn send_enabled(&self) -> bool {
        !self.state.disabled && !self.state.sending && !self.state.text.trim().is_empty()
    }
    pub fn cancel_enabled(&self) -> bool {
        self.state.sending
    }
    pub fn set_clipboard(&mut self, value: impl Into<String>) {
        self.clipboard = value.into();
    }

    pub fn set_model(&mut self, model: impl Into<String>) {
        self.state.model = Some(model.into());
    }

    pub fn set_agent(&mut self, agent: impl Into<String>) {
        self.state.agent = Some(agent.into());
    }

    pub fn file_references(&self) -> Vec<String> {
        self.state
            .text
            .split_whitespace()
            .filter(|word| word.starts_with('@') && word.len() > 1)
            .map(|word| word.trim_matches(|ch: char| matches!(ch, ',' | ';' | ')' | ']')))
            .map(str::to_owned)
            .collect()
    }

    pub fn record_history(&mut self, prompt: impl Into<String>) {
        let prompt = prompt.into();
        if !prompt.trim().is_empty() && self.history.last() != Some(&prompt) {
            self.history.push(prompt);
        }
        self.history_index = None;
    }

    pub fn history(&self) -> &[String] {
        &self.history
    }

    pub fn apply(&mut self, action: ComposerAction) -> bool {
        let editing = matches!(
            action,
            ComposerAction::Insert(_)
                | ComposerAction::Paste(_)
                | ComposerAction::Newline
                | ComposerAction::Backspace
                | ComposerAction::Delete
                | ComposerAction::MoveLeft { .. }
                | ComposerAction::MoveRight { .. }
                | ComposerAction::MoveHome { .. }
                | ComposerAction::MoveEnd { .. }
                | ComposerAction::SelectAll
                | ComposerAction::Copy
                | ComposerAction::Cut
                | ComposerAction::Clear
                | ComposerAction::InsertFileReference(_)
                | ComposerAction::Autocomplete(_)
                | ComposerAction::HistoryPrevious
                | ComposerAction::HistoryNext
        );
        if self.state.disabled && editing {
            return false;
        }
        let before = self.state.clone();
        match action {
            ComposerAction::Insert(s) | ComposerAction::Paste(s) => self.replace(s),
            ComposerAction::Newline => self.replace("\n".into()),
            ComposerAction::Backspace => self.delete(false),
            ComposerAction::Delete => self.delete(true),
            ComposerAction::MoveLeft { selecting } => {
                self.move_by(-1, selecting);
            }
            ComposerAction::MoveRight { selecting } => {
                self.move_by(1, selecting);
            }
            ComposerAction::MoveHome { selecting } => {
                self.edge(false, selecting);
            }
            ComposerAction::MoveEnd { selecting } => {
                self.edge(true, selecting);
            }
            ComposerAction::SelectAll => {
                self.state.selection = Selection {
                    anchor: 0,
                    cursor: self.state.text.len(),
                };
            }
            ComposerAction::Copy => {
                let (a, b) = ordered(self.state.selection);
                self.clipboard = self.state.text[a..b].to_owned();
            }
            ComposerAction::Cut => {
                let (a, b) = ordered(self.state.selection);
                self.clipboard = self.state.text[a..b].to_owned();
                self.replace(String::new());
            }
            ComposerAction::Clear => self.replace(String::new()),
            ComposerAction::InsertFileReference(path) => {
                self.replace(format!("@{}", Path::new(&path).display()))
            }
            ComposerAction::SlashQuery(q) => {
                self.state.slash_query = Some(q);
            }
            ComposerAction::Autocomplete(v) => self.replace(v),
            ComposerAction::OpenPicker(k) => {
                self.state.picker = Some(k);
            }
            ComposerAction::ChooseModel(v) => {
                self.state.model = Some(v);
                self.state.picker = None;
            }
            ComposerAction::ChooseAgent(v) => {
                self.state.agent = Some(v);
                self.state.picker = None;
            }
            ComposerAction::ClosePicker => {
                self.state.picker = None;
            }
            ComposerAction::SetDisabled(v) => {
                self.state.disabled = v;
            }
            ComposerAction::Send => {
                if self.send_enabled() {
                    self.state.sending = true;
                    self.state.disabled = true;
                }
            }
            ComposerAction::Cancel => {
                self.state.sending = false;
                self.state.disabled = false;
            }
            ComposerAction::SetSending(v) => {
                self.state.sending = v;
                self.state.disabled = v;
            }
            ComposerAction::SetConnection { connected, detail } => {
                self.state.connection = ConnectionStatus { connected, detail };
            }
            ComposerAction::Undo => return self.undo_redo(true),
            ComposerAction::Redo => return self.undo_redo(false),
            ComposerAction::HistoryPrevious => {
                let Some(next) = self.history_index.map_or_else(
                    || self.history.len().checked_sub(1),
                    |index| Some(index.saturating_sub(1)),
                ) else {
                    return false;
                };
                self.history_index = Some(next);
                self.state.text = self.history[next].clone();
                self.state.selection = Selection {
                    anchor: self.state.text.len(),
                    cursor: self.state.text.len(),
                };
            }
            ComposerAction::HistoryNext => {
                let Some(index) = self.history_index else {
                    return false;
                };
                if index + 1 < self.history.len() {
                    self.history_index = Some(index + 1);
                    self.state.text = self.history[index + 1].clone();
                } else {
                    self.history_index = None;
                    self.state.text.clear();
                }
                self.state.selection = Selection {
                    anchor: self.state.text.len(),
                    cursor: self.state.text.len(),
                };
            }
        };
        self.state.character_count = self.state.text.graphemes(true).count();
        let changed = self.state != before;
        if changed && (self.state.text != before.text) {
            self.undo.push(Snapshot {
                text: before.text,
                selection: before.selection,
            });
            self.redo.clear();
        }
        changed
    }
    fn replace(&mut self, value: String) {
        let (a, b) = ordered(self.state.selection);
        self.state.text.replace_range(a..b, &value);
        let p = a + value.len();
        self.state.selection = Selection {
            anchor: p,
            cursor: p,
        };
    }
    fn delete(&mut self, forward: bool) {
        let (a, b) = ordered(self.state.selection);
        if a != b {
            self.replace(String::new());
            return;
        }
        let p = self.state.selection.cursor;
        let q = if forward {
            next_boundary(&self.state.text, p)
        } else {
            prev_boundary(&self.state.text, p)
        };
        if q != p {
            let (x, y) = if forward { (p, q) } else { (q, p) };
            self.state.selection = Selection {
                anchor: x,
                cursor: y,
            };
            self.replace(String::new());
        }
    }
    fn move_by(&mut self, direction: i32, selecting: bool) -> bool {
        let p = self.state.selection.cursor;
        let q = if !selecting && !self.state.selection.is_empty() {
            let (start, end) = self.state.selection.range();
            if direction < 0 { start } else { end }
        } else if direction < 0 {
            prev_boundary(&self.state.text, p)
        } else {
            next_boundary(&self.state.text, p)
        };
        self.state.selection = if selecting {
            Selection {
                anchor: self.state.selection.anchor,
                cursor: q,
            }
        } else {
            Selection {
                anchor: q,
                cursor: q,
            }
        };
        true
    }
    fn edge(&mut self, end: bool, selecting: bool) -> bool {
        let p = if end { self.state.text.len() } else { 0 };
        self.state.selection = if selecting {
            Selection {
                anchor: self.state.selection.anchor,
                cursor: p,
            }
        } else {
            Selection {
                anchor: p,
                cursor: p,
            }
        };
        true
    }
    fn undo_redo(&mut self, undo: bool) -> bool {
        let stack = if undo { &mut self.undo } else { &mut self.redo };
        let Some(s) = stack.pop() else { return false };
        let current = Snapshot {
            text: self.state.text.clone(),
            selection: self.state.selection,
        };
        if undo {
            self.redo.push(current)
        } else {
            self.undo.push(current)
        }
        self.state.text = s.text;
        self.state.selection = s.selection;
        self.state.character_count = self.state.text.graphemes(true).count();
        true
    }
}

fn ordered(s: Selection) -> (usize, usize) {
    (s.anchor.min(s.cursor), s.anchor.max(s.cursor))
}
fn prev_boundary(t: &str, p: usize) -> usize {
    t[..p]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(i, _)| i)
}
fn next_boundary(t: &str, p: usize) -> usize {
    t[p..]
        .grapheme_indices(true)
        .next()
        .map_or(p, |(_, grapheme)| p + grapheme.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_graphemes_and_file_references_are_preserved() {
        let mut composer = Composer::new("session-1", "project-1");
        composer.apply(ComposerAction::Insert("e\u{301}👩‍💻 @src/lib.rs".into()));
        assert_eq!(composer.session_id(), "session-1");
        assert_eq!(composer.project_id(), "project-1");
        assert_eq!(composer.state().character_count, 14);
        assert_eq!(composer.file_references(), vec!["@src/lib.rs"]);
        composer.apply(ComposerAction::MoveHome { selecting: false });
        composer.apply(ComposerAction::MoveRight { selecting: true });
        composer.apply(ComposerAction::Copy);
        assert_eq!(composer.clipboard(), "e\u{301}");
    }

    #[test]
    fn history_picker_and_disabled_send_cancel_states_are_typed() {
        let mut composer = Composer::new("s", "p");
        composer.record_history("first");
        composer.record_history("second");
        composer.apply(ComposerAction::HistoryPrevious);
        assert_eq!(composer.text(), "second");
        composer.apply(ComposerAction::OpenPicker(PickerKind::Model));
        composer.apply(ComposerAction::ChooseModel("provider/model".into()));
        assert_eq!(composer.state().model.as_deref(), Some("provider/model"));
        assert_eq!(composer.state().picker, None);
        composer.apply(ComposerAction::Insert("prompt".into()));
        assert!(composer.send_enabled());
        composer.apply(ComposerAction::Send);
        assert!(composer.state().disabled);
        assert!(composer.cancel_enabled());
        composer.apply(ComposerAction::Cancel);
        assert!(!composer.state().disabled);
    }

    #[test]
    fn unicode_selection_and_connection_strip_data_are_stable() {
        let mut composer = Composer::new("s", "p");
        composer.apply(ComposerAction::Insert("😀x".into()));
        composer.apply(ComposerAction::MoveHome { selecting: false });
        composer.apply(ComposerAction::MoveRight { selecting: true });
        composer.apply(ComposerAction::Cut);
        assert_eq!(composer.clipboard(), "😀");
        assert_eq!(composer.text(), "x");
        composer.apply(ComposerAction::SetConnection {
            connected: true,
            detail: "local".into(),
        });
        assert_eq!(composer.state().connection.detail, "local");
    }
}
