//! Window-independent GPUI composer contract.
//!
//! The view can map GPUI actions and IME callbacks to [`ComposerAction`]
//! without duplicating editing or picker state.

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
struct Snapshot {
    text: String,
    selection: Selection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerAction {
    InsertText(String),
    Newline,
    Backspace,
    Delete,
    MoveLeft { selecting: bool },
    MoveRight { selecting: bool },
    MoveUp { selecting: bool },
    MoveDown { selecting: bool },
    MoveHome { selecting: bool },
    MoveEnd { selecting: bool },
    SelectAll,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    HistoryPrevious,
    HistoryNext,
    InsertFileReference(String),
    ChooseSlashCommand(String),
    ChooseCompletion(String),
    ChooseModel(String),
    ChooseAgent(String),
    Send,
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerMode {
    Editing,
    Streaming,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerState {
    pub mode: ComposerMode,
    pub disabled: bool,
    pub send_enabled: bool,
    pub cancel_enabled: bool,
}

impl ComposerState {
    pub fn editing() -> Self {
        Self {
            mode: ComposerMode::Editing,
            disabled: false,
            send_enabled: false,
            cancel_enabled: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Composer {
    session_id: Option<String>,
    project_id: Option<String>,
    text: String,
    selection: Selection,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    history: Vec<String>,
    history_index: Option<usize>,
    clipboard: String,
    state: ComposerState,
    pub model: Option<String>,
    pub agent: Option<String>,
}

impl Default for Composer {
    fn default() -> Self {
        Self::new()
    }
}

impl Composer {
    pub fn new() -> Self {
        Self {
            session_id: None,
            project_id: None,
            text: String::new(),
            selection: Selection::caret(0),
            undo: Vec::new(),
            redo: Vec::new(),
            history: Vec::new(),
            history_index: None,
            clipboard: String::new(),
            state: ComposerState::editing(),
            model: None,
            agent: None,
        }
    }

    /// Attach the IDs supplied by the surrounding session/project integration.
    /// The composer stores them opaquely and never invents protocol requests.
    pub fn with_ids(session_id: impl Into<String>, project_id: impl Into<String>) -> Self {
        let mut composer = Self::new();
        composer.session_id = Some(session_id.into());
        composer.project_id = Some(project_id.into());
        composer
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub fn project_id(&self) -> Option<&str> {
        self.project_id.as_deref()
    }

    pub fn set_ids(&mut self, session_id: impl Into<String>, project_id: impl Into<String>) {
        self.session_id = Some(session_id.into());
        self.project_id = Some(project_id.into());
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn selection(&self) -> Selection {
        self.selection
    }

    /// Synchronize the GPUI IME adapter after an edit made by its native
    /// `EntityInputHandler`.  All subsequent actions still go through this
    /// model; this is intentionally not a second editable buffer.
    pub fn sync_ime_text(&mut self, text: impl Into<String>) {
        let text = text.into();
        if self.text != text {
            self.text = text;
            self.selection = Selection::caret(self.text.len());
            self.undo.clear();
            self.redo.clear();
            self.state.send_enabled = !self.state.disabled && !self.text.trim().is_empty();
        }
    }

    pub fn selected_text(&self) -> Option<&str> {
        let (start, end) = self.safe_range();
        (!self.selection.is_empty()).then(|| &self.text[start..end])
    }

    pub fn character_count(&self) -> usize {
        self.text.graphemes(true).count()
    }

    pub fn state(&self) -> &ComposerState {
        &self.state
    }

    pub fn clipboard(&self) -> &str {
        &self.clipboard
    }

    pub fn set_clipboard(&mut self, value: impl Into<String>) {
        self.clipboard = value.into();
    }

    pub fn set_streaming(&mut self, streaming: bool) {
        self.state.mode = if streaming {
            ComposerMode::Streaming
        } else {
            ComposerMode::Editing
        };
        self.state.disabled = streaming;
        self.state.cancel_enabled = streaming;
        self.state.send_enabled = !streaming && !self.text.trim().is_empty();
    }

    pub fn set_model(&mut self, model: impl Into<String>) {
        self.model = Some(model.into());
    }

    pub fn set_agent(&mut self, agent: impl Into<String>) {
        self.agent = Some(agent.into());
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

    pub fn file_references(&self) -> Vec<String> {
        self.text
            .split_whitespace()
            .filter(|word| word.starts_with('@') && word.len() > 1)
            .map(|word| word.trim_matches(|ch: char| matches!(ch, ',' | ';' | ')' | ']')))
            .map(str::to_owned)
            .collect()
    }

    pub fn apply(&mut self, action: ComposerAction) -> Option<String> {
        if self.state.disabled && !matches!(&action, ComposerAction::Cancel) {
            return None;
        }
        match action {
            ComposerAction::InsertText(value) => self.replace(&value),
            ComposerAction::Newline => self.replace("\n"),
            ComposerAction::Backspace => self.delete(false),
            ComposerAction::Delete => self.delete(true),
            ComposerAction::MoveLeft { selecting } => self.move_horizontal(-1, selecting),
            ComposerAction::MoveRight { selecting } => self.move_horizontal(1, selecting),
            ComposerAction::MoveUp { selecting } => self.move_vertical(-1, selecting),
            ComposerAction::MoveDown { selecting } => self.move_vertical(1, selecting),
            ComposerAction::MoveHome { selecting } => self.move_line_edge(false, selecting),
            ComposerAction::MoveEnd { selecting } => self.move_line_edge(true, selecting),
            ComposerAction::SelectAll => {
                self.selection = Selection {
                    anchor: 0,
                    cursor: self.text.len(),
                };
            }
            ComposerAction::Undo => self.undo_redo(true),
            ComposerAction::Redo => self.undo_redo(false),
            ComposerAction::Cut => {
                self.clipboard = self.selected_text().unwrap_or_default().to_owned();
                self.replace("");
            }
            ComposerAction::Copy => {
                self.clipboard = self.selected_text().unwrap_or_default().to_owned();
            }
            ComposerAction::Paste => {
                let value = self.clipboard.clone();
                self.replace(&value);
            }
            ComposerAction::HistoryPrevious => self.history_previous(),
            ComposerAction::HistoryNext => self.history_next(),
            ComposerAction::InsertFileReference(path) => self.replace(&format!("@{path}")),
            ComposerAction::ChooseSlashCommand(value) | ComposerAction::ChooseCompletion(value) => {
                self.replace_completion(&value)
            }
            ComposerAction::ChooseModel(value) => self.set_model(value),
            ComposerAction::ChooseAgent(value) => self.set_agent(value),
            ComposerAction::Send => {
                if self.state.send_enabled {
                    let value = self.text.clone();
                    self.record_history(value.clone());
                    self.set_streaming(true);
                    return Some(value);
                }
            }
            ComposerAction::Cancel => self.set_streaming(false),
        }
        self.state.send_enabled = !self.state.disabled && !self.text.trim().is_empty();
        None
    }

    fn replace(&mut self, value: &str) {
        let (start, end) = self.safe_range();
        self.undo.push(Snapshot {
            text: self.text.clone(),
            selection: self.selection,
        });
        self.redo.clear();
        self.text.replace_range(start..end, value);
        self.selection = Selection::caret(start + value.len());
    }

    fn replace_completion(&mut self, value: &str) {
        let start = self.text[..self.selection.cursor]
            .rfind(char::is_whitespace)
            .map_or(0, |index| index + 1);
        self.selection.anchor = start;
        self.replace(value);
    }

    fn safe_range(&self) -> (usize, usize) {
        let (start, end) = self.selection.range();
        (
            previous_boundary(&self.text, start),
            previous_boundary(&self.text, end),
        )
    }

    fn delete(&mut self, forward: bool) {
        if self.selection.is_empty() {
            let position = if forward {
                self.boundary_right(self.selection.cursor)
            } else {
                self.boundary_left(self.selection.cursor)
            };
            self.selection = if forward {
                Selection {
                    anchor: self.selection.cursor,
                    cursor: position,
                }
            } else {
                Selection {
                    anchor: position,
                    cursor: self.selection.cursor,
                }
            };
        }
        self.replace("");
    }

    fn move_horizontal(&mut self, direction: i8, selecting: bool) {
        let position = if !selecting && !self.selection.is_empty() {
            let (start, end) = self.selection.range();
            if direction < 0 { start } else { end }
        } else if direction < 0 {
            self.boundary_left(self.selection.cursor)
        } else {
            self.boundary_right(self.selection.cursor)
        };
        self.move_to(position, selecting);
    }

    fn move_line_edge(&mut self, end: bool, selecting: bool) {
        let start = self.text[..self.selection.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let line_end = self.text[self.selection.cursor..]
            .find('\n')
            .map_or(self.text.len(), |index| self.selection.cursor + index);
        self.move_to(if end { line_end } else { start }, selecting);
    }

    fn move_vertical(&mut self, direction: i8, selecting: bool) {
        let line_start = self.text[..self.selection.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let line_end = self.text[self.selection.cursor..]
            .find('\n')
            .map_or(self.text.len(), |index| self.selection.cursor + index);
        let column = self.text[line_start..self.selection.cursor]
            .graphemes(true)
            .count();
        let position = if direction < 0 {
            if line_start == 0 {
                0
            } else {
                let end = line_start - 1;
                let start = self.text[..end].rfind('\n').map_or(0, |index| index + 1);
                grapheme_at_column(&self.text, start, end, column)
            }
        } else if line_end == self.text.len() {
            line_end
        } else {
            let start = line_end + 1;
            let end = self.text[start..]
                .find('\n')
                .map_or(self.text.len(), |index| start + index);
            grapheme_at_column(&self.text, start, end, column)
        };
        self.move_to(position, selecting);
    }

    fn move_to(&mut self, position: usize, selecting: bool) {
        if selecting {
            self.selection.anchor = self.selection.anchor.min(self.text.len());
        } else {
            self.selection.anchor = position;
        }
        self.selection.cursor = position;
    }

    fn boundary_left(&self, at: usize) -> usize {
        self.text[..at]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn boundary_right(&self, at: usize) -> usize {
        self.text[at..]
            .graphemes(true)
            .next()
            .map_or(at, |grapheme| at + grapheme.len())
    }

    fn undo_redo(&mut self, undo: bool) {
        let stack = if undo { &mut self.undo } else { &mut self.redo };
        let Some(snapshot) = stack.pop() else { return };
        let current = Snapshot {
            text: self.text.clone(),
            selection: self.selection,
        };
        if undo {
            self.redo.push(current);
        } else {
            self.undo.push(current);
        }
        self.text = snapshot.text;
        self.selection = snapshot.selection;
    }

    fn history_previous(&mut self) {
        let Some(index) = self.history_index.map_or_else(
            || self.history.len().checked_sub(1),
            |index| Some(index.saturating_sub(1)),
        ) else {
            return;
        };
        self.history_index = Some(index);
        self.text = self.history[index].clone();
        self.selection = Selection::caret(self.text.len());
    }

    fn history_next(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 < self.history.len() {
            self.history_index = Some(index + 1);
            self.text = self.history[index + 1].clone();
        } else {
            self.history_index = None;
            self.text.clear();
        }
        self.selection = Selection::caret(self.text.len());
    }
}

fn grapheme_at_column(text: &str, start: usize, end: usize, column: usize) -> usize {
    text[start..end]
        .grapheme_indices(true)
        .nth(column)
        .map_or(end, |(index, _)| start + index)
}

fn previous_boundary(text: &str, at: usize) -> usize {
    let at = at.min(text.len());
    if text.is_char_boundary(at) {
        at
    } else {
        text[..at].char_indices().next_back().map_or(0, |(i, _)| i)
    }
}

pub fn command_suggestions(input: &str, commands: &[String]) -> Vec<String> {
    let prefix = input.split_whitespace().last().unwrap_or(input);
    commands
        .iter()
        .filter(|command| command.starts_with(prefix))
        .cloned()
        .collect()
}

pub trait Clipboard {
    fn read(&self) -> Option<String>;
    fn write(&mut self, text: &str);
}

pub trait ComposerListener {
    fn on_action(&mut self, composer: &mut Composer, action: ComposerAction);
}

pub trait ImeTarget {
    fn ime_text(&self) -> &str;
    fn ime_selection(&self) -> Selection;
}

impl ImeTarget for Composer {
    fn ime_text(&self) -> &str {
        self.text()
    }

    fn ime_selection(&self) -> Selection {
        self.selection()
    }
}

pub fn listener_for_ime() -> impl ComposerListener {
    struct Listener;
    impl ComposerListener for Listener {
        fn on_action(&mut self, composer: &mut Composer, action: ComposerAction) {
            composer.apply(action);
        }
    }
    Listener
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grapheme_selection_and_vertical_motion_are_safe() {
        let mut composer = Composer::new();
        composer.apply(ComposerAction::InsertText("a👩‍💻\nxy".into()));
        composer.apply(ComposerAction::MoveHome { selecting: false });
        composer.apply(ComposerAction::MoveUp { selecting: false });
        composer.apply(ComposerAction::MoveRight { selecting: true });
        assert_eq!(composer.selected_text(), Some("a"));
        assert_eq!(composer.character_count(), 5);
    }

    #[test]
    fn clipboard_history_file_refs_and_streaming_states_work() {
        let mut composer = Composer::with_ids("session-1", "project-1");
        assert_eq!(composer.session_id(), Some("session-1"));
        assert_eq!(composer.project_id(), Some("project-1"));
        composer.apply(ComposerAction::InsertText("@src/lib.rs".into()));
        assert_eq!(composer.file_references(), vec!["@src/lib.rs"]);
        composer.apply(ComposerAction::SelectAll);
        composer.apply(ComposerAction::Copy);
        assert_eq!(composer.clipboard(), "@src/lib.rs");
        composer.record_history("previous");
        composer.apply(ComposerAction::HistoryPrevious);
        assert_eq!(composer.text(), "previous");
        let sent = composer.apply(ComposerAction::Send);
        assert_eq!(sent.as_deref(), Some("previous"));
        assert!(composer.state().disabled);
        composer.apply(ComposerAction::Cancel);
        assert!(!composer.state().disabled);
    }

    #[test]
    fn ime_listener_is_reachable_and_commands_are_typed() {
        let mut composer = Composer::new();
        let mut listener = listener_for_ime();
        listener.on_action(&mut composer, ComposerAction::InsertText("/mo".into()));
        assert_eq!(composer.ime_text(), "/mo");
        assert_eq!(
            command_suggestions("/mo", &["/model".into()]),
            vec!["/model"]
        );
    }
}
