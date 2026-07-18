use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, IntoElement, KeyBinding, LayoutId,
    PaintQuad, Pixels, Render, ShapedLine, Style, TextRun, UTF16Selection, UnderlineStyle, Window,
    fill, point, prelude::*, px, relative, rgb,
};
use unicode_segmentation::UnicodeSegmentation;

/// Pure editing core kept independent of GPUI, so scripted fixtures can test
/// Unicode, multiline movement and selection without opening a window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditorBuffer {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
}

impl EditorBuffer {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.len();
        Self {
            text,
            cursor,
            anchor: None,
        }
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    pub fn selection(&self) -> Option<Range<usize>> {
        self.anchor.map(|a| a.min(self.cursor)..a.max(self.cursor))
    }
    pub fn insert(&mut self, value: &str) {
        self.delete_selection();
        self.text.insert_str(self.cursor, value);
        self.cursor += value.len();
    }
    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if let Some(start) = previous_grapheme(&self.text, self.cursor) {
            self.text.replace_range(start..self.cursor, "");
            self.cursor = start;
        }
    }
    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor < self.text.len() {
            let end = next_grapheme(&self.text, self.cursor);
            self.text.replace_range(self.cursor..end, "");
        }
    }
    pub fn move_left(&mut self, selecting: bool) {
        self.move_to(
            previous_grapheme(&self.text, self.cursor).unwrap_or(0),
            selecting,
        );
    }
    pub fn move_right(&mut self, selecting: bool) {
        self.move_to(next_grapheme(&self.text, self.cursor), selecting);
    }
    pub fn move_home(&mut self, selecting: bool) {
        let start = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
        self.move_to(start, selecting);
    }
    pub fn move_end(&mut self, selecting: bool) {
        let end = self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |i| self.cursor + i);
        self.move_to(end, selecting);
    }
    pub fn move_up(&mut self, selecting: bool) {
        let line_start = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
        if line_start == 0 {
            return self.move_to(0, selecting);
        }
        let column = self.text[line_start..self.cursor].graphemes(true).count();
        let prior_end = line_start - 1;
        let prior_start = self.text[..prior_end].rfind('\n').map_or(0, |i| i + 1);
        self.move_to(
            grapheme_at_column(&self.text, prior_start, prior_end, column),
            selecting,
        );
    }
    pub fn move_down(&mut self, selecting: bool) {
        let end = self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |i| self.cursor + i);
        if end == self.text.len() {
            return self.move_to(end, selecting);
        }
        let start = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
        let column = self.text[start..self.cursor].graphemes(true).count();
        let next_start = end + 1;
        let next_end = self.text[next_start..]
            .find('\n')
            .map_or(self.text.len(), |i| next_start + i);
        self.move_to(
            grapheme_at_column(&self.text, next_start, next_end, column),
            selecting,
        );
    }
    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|r| &self.text[r])
    }
    fn move_to(&mut self, position: usize, selecting: bool) {
        if selecting {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = position;
    }
    fn replace_range(&mut self, range: Range<usize>, value: &str) {
        self.text.replace_range(range.clone(), value);
        self.cursor = range.start + value.len();
        self.anchor = None;
    }
    fn set_cursor(&mut self, cursor: usize) {
        self.cursor = cursor;
        self.anchor = None;
    }
    fn delete_selection(&mut self) -> bool {
        if let Some(range) = self.selection() {
            self.text.replace_range(range.clone(), "");
            self.cursor = range.start;
            self.anchor = None;
            true
        } else {
            false
        }
    }
}

fn previous_grapheme(s: &str, at: usize) -> Option<usize> {
    s[..at]
        .grapheme_indices(true)
        .next_back()
        .map(|(index, _)| index)
}
fn next_grapheme(s: &str, at: usize) -> usize {
    s[at..]
        .grapheme_indices(true)
        .next()
        .map_or(at, |(_, grapheme)| at + grapheme.len())
}
fn grapheme_at_column(s: &str, start: usize, end: usize, col: usize) -> usize {
    let line = &s[start..end];
    line.grapheme_indices(true)
        .nth(col)
        .map_or(end, |(offset, _)| start + offset)
}

#[cfg(test)]
mod editor_tests {
    use super::*;
    #[test]
    fn unicode_and_multiline_selection() {
        let mut e = EditorBuffer::new("a😀");
        e.move_home(false);
        assert_eq!(e.cursor(), 0);
        e.move_right(true);
        e.move_right(true);
        assert_eq!(e.selected_text(), Some("a😀"));
        e.insert("ok");
        assert_eq!(e.text(), "ok");
    }

    #[test]
    fn grapheme_navigation_keeps_emoji_sequences_and_combining_marks_together() {
        let mut e = EditorBuffer::new("e\u{301}👩‍💻x");
        e.move_home(false);
        e.move_right(false);
        assert_eq!(e.selected_text(), None);
        assert_eq!(e.cursor(), "e\u{301}".len());
        e.move_right(false);
        assert_eq!(e.cursor(), "e\u{301}👩‍💻".len());
        e.backspace();
        assert_eq!(e.text(), "e\u{301}x");
    }

    #[test]
    fn vertical_motion_uses_grapheme_columns() {
        let mut e = EditorBuffer::new("a👩‍💻c\nxy\nz");
        e.move_home(false);
        e.move_up(false);
        e.move_up(false);
        e.move_right(false);
        e.move_right(false);
        e.move_down(false);
        assert_eq!(&e.text()[e.cursor()..], "\nz");
    }

    #[test]
    fn selection_replaces_a_whole_grapheme_in_reverse() {
        let mut e = EditorBuffer::new("a👩‍💻b");
        e.move_home(false);
        e.move_right(false);
        e.move_right(true);
        assert_eq!(e.selected_text(), Some("👩‍💻"));
        e.insert("界");
        assert_eq!(e.text(), "a界b");
    }

    #[test]
    fn delete_and_backspace_respect_grapheme_boundaries() {
        let mut e = EditorBuffer::new("e\u{301}👩‍💻x");
        e.move_end(false);
        e.backspace();
        assert_eq!(e.text(), "e\u{301}👩‍💻");
        e.move_home(false);
        e.delete();
        assert_eq!(e.text(), "👩‍💻");
    }

    #[test]
    fn deletion_is_noop_at_both_unicode_boundaries() {
        let mut e = EditorBuffer::new("😀x");
        e.move_home(false);
        e.backspace();
        e.delete();
        assert_eq!(e.text(), "x");
        e.move_end(false);
        e.delete();
        e.delete();
        assert_eq!(e.text(), "x");
        e.backspace();
        e.backspace();
        e.backspace();
        assert_eq!(e.text(), "");
    }

    #[test]
    fn selection_delete_and_backspace_collapse_to_selection_start() {
        let mut e = EditorBuffer::new("ab😀cd");
        e.move_home(false);
        e.move_right(true);
        e.move_right(true);
        e.move_right(true);
        assert_eq!(e.selected_text(), Some("ab😀"));
        e.delete();
        assert_eq!(e.text(), "cd");
        assert_eq!(e.cursor(), 0);

        e.move_end(false);
        e.move_left(true);
        e.backspace();
        assert_eq!(e.text(), "c");
        assert_eq!(e.cursor(), 1);
        assert_eq!(e.selection(), None);
    }

    #[test]
    fn horizontal_selection_can_be_reversed_without_invalid_ranges() {
        let mut e = EditorBuffer::new("a😀b");
        e.move_end(false);
        e.move_left(true);
        e.move_left(true);
        assert_eq!(e.selected_text(), Some("😀b"));
        e.move_left(false);
        assert_eq!(e.selection(), None);
        assert_eq!(e.cursor(), 0);
    }
}

/// Whether the prompt should be interpreted as a command rather than a chat
/// message.  This deliberately only inspects the first non-whitespace byte;
/// command arguments may contain arbitrary Unicode.
pub fn command_mode(input: &str) -> bool {
    input.trim_start().starts_with('/')
}

/// The keyboard outcomes a view can map to its own actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputAction {
    Submit,
    Dismiss,
    MoveUp,
    MoveDown,
    AcceptCompletion,
    NextCompletion,
}

/// Translate the prompt's conventional keys without coupling this module to
/// any particular GPUI action type. The view can bind these outcomes to its
/// picker/navigation actions.
pub fn input_action(key: &str, completion_visible: bool) -> Option<InputAction> {
    match key {
        "enter" => Some(if completion_visible {
            InputAction::AcceptCompletion
        } else {
            InputAction::Submit
        }),
        "escape" => Some(InputAction::Dismiss),
        "up" => Some(InputAction::MoveUp),
        "down" => Some(InputAction::MoveDown),
        "tab" => Some(InputAction::NextCompletion),
        _ => None,
    }
}

/// Pure state for command argument completion.  The view owns this value and
/// supplies the command's available arguments; input editing remains owned by
/// `TextInput`/`EditorBuffer`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AutocompleteState {
    matches: Vec<String>,
    selected: usize,
}

impl AutocompleteState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn matches(&self) -> &[String] {
        &self.matches
    }
    pub fn selected(&self) -> Option<&str> {
        self.matches.get(self.selected).map(String::as_str)
    }
    pub fn is_visible(&self) -> bool {
        !self.matches.is_empty()
    }

    /// Refresh suggestions for the current command argument. Matching is
    /// case-insensitive and prefix-based, preserving caller ordering.
    pub fn update(&mut self, input: &str, candidates: &[String]) {
        self.matches.clear();
        self.selected = 0;
        if !command_mode(input) {
            return;
        }
        let token = input.split_whitespace().last().unwrap_or("");
        let needle = token.to_lowercase();
        self.matches.extend(
            candidates
                .iter()
                .filter(|candidate| candidate.to_lowercase().starts_with(&needle))
                .cloned(),
        );
    }

    pub fn dismiss(&mut self) {
        self.matches.clear();
        self.selected = 0;
    }

    pub fn cycle(&mut self, direction: i32) -> Option<&str> {
        if self.matches.is_empty() {
            return None;
        }
        let len = self.matches.len() as i32;
        self.selected = (self.selected as i32 + direction).rem_euclid(len) as usize;
        self.selected()
    }

    /// Return the selected value and replace only the final argument token.
    /// The returned string is safe for UTF-8 input and retains preceding text.
    pub fn accept(&self, input: &str) -> Option<String> {
        let value = self.selected()?.to_owned();
        let end = input.len();
        let start = input[..end]
            .char_indices()
            .rev()
            .find(|(_, ch)| ch.is_whitespace())
            .map_or(0, |(i, ch)| i + ch.len_utf8());
        Some(format!("{}{}", &input[..start], value))
    }
}

#[cfg(test)]
mod autocomplete_tests {
    use super::*;

    fn candidates() -> Vec<String> {
        ["Plan😀", "Planner", "Build"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn slash_reaches_command_mode() {
        assert!(command_mode("/agent "));
        assert!(command_mode("  /model"));
        assert!(!command_mode("hello /agent"));
    }

    #[test]
    fn autocomplete_filters_cycles_and_accepts_unicode() {
        let mut state = AutocompleteState::new();
        state.update("/agent pla", &candidates());
        assert_eq!(state.matches(), &["Plan😀", "Planner"]);
        assert_eq!(state.selected(), Some("Plan😀"));
        assert_eq!(state.cycle(1), Some("Planner"));
        assert_eq!(state.cycle(1), Some("Plan😀"));
        assert_eq!(state.accept("/agent pla"), Some("/agent Plan😀".into()));
    }

    #[test]
    fn dismissal_hides_and_clears_completion() {
        let mut state = AutocompleteState::new();
        state.update("/agent b", &candidates());
        assert!(state.is_visible());
        state.dismiss();
        assert!(!state.is_visible());
        assert_eq!(state.selected(), None);
        assert_eq!(state.cycle(1), None);
    }
}

pub struct TextInput {
    focus_handle: FocusHandle,
    buffer: EditorBuffer,
    history: Vec<String>,
    history_index: Option<usize>,
    disabled: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_lines: Vec<(usize, ShapedLine)>,
    last_bounds: Option<Bounds<Pixels>>,
}

impl TextInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            buffer: EditorBuffer::default(),
            history: Vec::new(),
            history_index: None,
            disabled: false,
            marked_range: None,
            last_layout: None,
            last_lines: Vec::new(),
            last_bounds: None,
        }
    }

    pub fn content(&self) -> String {
        self.buffer.text().to_owned()
    }

    pub fn reset(&mut self) {
        self.buffer = EditorBuffer::default();
        self.history_index = None;
        self.marked_range = None;
    }

    pub fn set_disabled(&mut self, disabled: bool) {
        self.disabled = disabled;
    }

    pub fn record_submission(&mut self, text: impl Into<String>) {
        let text = text.into();
        if text.trim().is_empty() {
            return;
        }
        if self.history.last() != Some(&text) {
            self.history.push(text);
        }
        self.history_index = None;
    }

    pub fn history(&self) -> &[String] {
        &self.history
    }

    fn history_previous(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next = self
            .history_index
            .map_or(self.history.len() - 1, |index| index.saturating_sub(1));
        self.history_index = Some(next);
        self.buffer = EditorBuffer::new(self.history[next].clone());
    }

    fn history_next(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 < self.history.len() {
            let next = index + 1;
            self.history_index = Some(next);
            self.buffer = EditorBuffer::new(self.history[next].clone());
        } else {
            self.history_index = None;
            self.buffer = EditorBuffer::default();
        }
    }

    /// Replace the prompt text while preserving the input entity and focus.
    /// Picker views use this instead of reaching into the editor's storage.
    pub fn set_content(&mut self, content: impl Into<String>) {
        self.buffer = EditorBuffer::new(content);
        self.history_index = None;
        self.marked_range = None;
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        self.buffer.backspace();
        cx.notify();
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if !self.disabled {
            self.buffer.delete();
            cx.notify();
        }
    }

    fn move_left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_left(false);
        cx.notify();
    }

    fn move_right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_right(false);
        cx.notify();
    }

    fn move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_up(false);
        cx.notify();
    }

    fn move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_down(false);
        cx.notify();
    }

    fn move_home(&mut self, _: &MoveHome, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_home(false);
        cx.notify();
    }

    fn move_end(&mut self, _: &MoveEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_end(false);
        cx.notify();
    }

    fn move_left_selecting(
        &mut self,
        _: &MoveLeftSelecting,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.buffer.move_left(true);
        cx.notify();
    }

    fn move_right_selecting(
        &mut self,
        _: &MoveRightSelecting,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.buffer.move_right(true);
        cx.notify();
    }

    fn move_up_selecting(&mut self, _: &MoveUpSelecting, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_up(true);
        cx.notify();
    }

    fn move_down_selecting(
        &mut self,
        _: &MoveDownSelecting,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.buffer.move_down(true);
        cx.notify();
    }

    fn move_home_selecting(
        &mut self,
        _: &MoveHomeSelecting,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.buffer.move_home(true);
        cx.notify();
    }

    fn move_end_selecting(&mut self, _: &MoveEndSelecting, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_end(true);
        cx.notify();
    }

    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        if !self.disabled {
            self.buffer.insert("\n");
            cx.notify();
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.buffer.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
        }
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        if let Some(text) = self.buffer.selected_text().map(str::to_owned) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.buffer.delete();
            cx.notify();
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.buffer.insert(&text);
            cx.notify();
        }
    }

    fn history_previous_action(
        &mut self,
        _: &HistoryPrevious,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.history_previous();
        cx.notify();
    }

    fn history_next_action(&mut self, _: &HistoryNext, _: &mut Window, cx: &mut Context<Self>) {
        self.history_next();
        cx.notify();
    }
}

gpui::actions!(
    tau_input,
    [
        Backspace,
        Delete,
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        MoveHome,
        MoveEnd,
        MoveLeftSelecting,
        MoveRightSelecting,
        MoveUpSelecting,
        MoveDownSelecting,
        MoveHomeSelecting,
        MoveEndSelecting,
        Newline,
        Copy,
        Cut,
        Paste,
        HistoryPrevious,
        HistoryNext
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("TauPromptInput")),
        KeyBinding::new("delete", Delete, Some("TauPromptInput")),
        KeyBinding::new("left", MoveLeft, Some("TauPromptInput")),
        KeyBinding::new("shift-left", MoveLeftSelecting, Some("TauPromptInput")),
        KeyBinding::new("right", MoveRight, Some("TauPromptInput")),
        KeyBinding::new("shift-right", MoveRightSelecting, Some("TauPromptInput")),
        KeyBinding::new("up", MoveUp, Some("TauPromptInput")),
        KeyBinding::new("shift-up", MoveUpSelecting, Some("TauPromptInput")),
        KeyBinding::new("down", MoveDown, Some("TauPromptInput")),
        KeyBinding::new("shift-down", MoveDownSelecting, Some("TauPromptInput")),
        KeyBinding::new("home", MoveHome, Some("TauPromptInput")),
        KeyBinding::new("shift-home", MoveHomeSelecting, Some("TauPromptInput")),
        KeyBinding::new("end", MoveEnd, Some("TauPromptInput")),
        KeyBinding::new("shift-end", MoveEndSelecting, Some("TauPromptInput")),
        KeyBinding::new("shift-enter", Newline, Some("TauPromptInput")),
        KeyBinding::new("cmd-c", Copy, Some("TauPromptInput")),
        KeyBinding::new("ctrl-c", Copy, Some("TauPromptInput")),
        KeyBinding::new("cmd-x", Cut, Some("TauPromptInput")),
        KeyBinding::new("ctrl-x", Cut, Some("TauPromptInput")),
        KeyBinding::new("cmd-v", Paste, Some("TauPromptInput")),
        KeyBinding::new("ctrl-v", Paste, Some("TauPromptInput")),
        KeyBinding::new("alt-up", HistoryPrevious, Some("TauPromptInput")),
        KeyBinding::new("alt-down", HistoryNext, Some("TauPromptInput")),
    ]);
}

impl Render for TextInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::div()
            .key_context("TauPromptInput")
            .track_focus(&self.focus_handle(cx))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::move_left))
            .on_action(cx.listener(Self::move_right))
            .on_action(cx.listener(Self::move_up))
            .on_action(cx.listener(Self::move_down))
            .on_action(cx.listener(Self::move_home))
            .on_action(cx.listener(Self::move_end))
            .on_action(cx.listener(Self::move_left_selecting))
            .on_action(cx.listener(Self::move_right_selecting))
            .on_action(cx.listener(Self::move_up_selecting))
            .on_action(cx.listener(Self::move_down_selecting))
            .on_action(cx.listener(Self::move_home_selecting))
            .on_action(cx.listener(Self::move_end_selecting))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::history_previous_action))
            .on_action(cx.listener(Self::history_next_action))
            .cursor(gpui::CursorStyle::IBeam)
            .flex()
            .w_full()
            .min_h(px(72.))
            .p(px(12.))
            .bg(rgb(0x1b1f27))
            .border_1()
            .border_color(rgb(0x394354))
            .rounded_lg()
            .when(self.disabled, |element| element.opacity(0.6))
            .child(TextElement { input: cx.entity() })
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.utf8_range(range);
        adjusted.replace(self.utf16_range(range.clone()));
        Some(self.buffer.text()[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let cursor = self.buffer.text()[..self.buffer.cursor()]
            .encode_utf16()
            .count();
        let selection = self.buffer.selection();
        let range = selection.as_ref().map_or(cursor..cursor, |range| {
            self.buffer.text()[..range.start].encode_utf16().count()
                ..self.buffer.text()[..range.end].encode_utf16().count()
        });
        Some(UTF16Selection {
            range,
            reversed: self
                .buffer
                .selection()
                .is_some_and(|range| self.buffer.cursor() == range.start),
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.utf16_range(range.clone()))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        let range = range
            .map(|range| self.utf8_range(range))
            .or_else(|| self.marked_range.clone())
            .or_else(|| self.buffer.selection())
            .unwrap_or_else(|| self.buffer.cursor()..self.buffer.cursor());
        self.buffer.replace_range(range, text);
        self.marked_range = None;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        new_selected_range: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|range| self.utf8_range(range))
            .or_else(|| self.marked_range.clone())
            .or_else(|| self.buffer.selection())
            .unwrap_or_else(|| self.buffer.cursor()..self.buffer.cursor());
        if self.disabled {
            return;
        }
        self.buffer.replace_range(range.clone(), text);
        self.marked_range = (!text.is_empty()).then_some(range.start..range.start + text.len());
        if let Some(selected) = new_selected_range {
            let cursor = range.start + utf8_offset(text, selected.end).min(text.len());
            self.buffer.set_cursor(cursor.min(self.buffer.text().len()));
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.utf8_range(range);
        let line_height = px(bounds.size.height / px(self.last_lines.len().max(1) as f32));
        let (line_start, line) = self
            .last_lines
            .iter()
            .find(|(start, line)| range.start <= *start + line.len())?;
        let line_index = self
            .last_lines
            .iter()
            .position(|(start, _)| start == line_start)
            .unwrap_or(0);
        let start = range.start.saturating_sub(*line_start);
        let end = range.end.saturating_sub(*line_start).min(line.len());
        Some(Bounds::from_corners(
            point(
                bounds.left() + line.x_for_index(start),
                bounds.top() + line_height * line_index as f32,
            ),
            point(
                bounds.left() + line.x_for_index(end),
                bounds.top() + line_height * (line_index + 1) as f32,
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let line_height = px(bounds.size.height / px(self.last_lines.len().max(1) as f32));
        let line_index = ((point.y - bounds.top()) / line_height).floor().max(0.) as usize;
        let (line_start, line) = self
            .last_lines
            .get(line_index.min(self.last_lines.len().saturating_sub(1)))?;
        let index = line.closest_index_for_x(point.x - bounds.left());
        Some(
            self.buffer.text()[..(*line_start + index).min(self.buffer.text().len())]
                .encode_utf16()
                .count(),
        )
    }
}

impl TextInput {
    fn utf8_range(&self, range: Range<usize>) -> Range<usize> {
        let start = self.utf8_offset(range.start);
        let end = self.utf8_offset(range.end);
        start..end
    }

    fn utf16_range(&self, range: Range<usize>) -> Range<usize> {
        self.buffer.text()[..range.start].encode_utf16().count()
            ..self.buffer.text()[..range.end].encode_utf16().count()
    }

    fn utf8_offset(&self, offset: usize) -> usize {
        utf8_offset(self.buffer.text(), offset)
    }
}

fn utf8_offset(text: &str, offset: usize) -> usize {
    let mut utf16 = 0;
    for (index, ch) in text.char_indices() {
        if utf16 >= offset {
            return index;
        }
        utf16 += ch.len_utf16();
    }
    text.len()
}

struct TextElement {
    input: Entity<TextInput>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

struct TextPrepaint {
    lines: Vec<(usize, ShapedLine)>,
    cursor: PaintQuad,
    selection: Vec<PaintQuad>,
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = TextPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let line_count = self.input.read(cx).buffer.text().split('\n').count();
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = (window.line_height() * line_count as f32).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let text = input.buffer.text();
        let mut lines = Vec::new();
        let mut line_start = 0;
        for line_text in text.split('\n') {
            let content: gpui::SharedString = line_text.to_owned().into();
            let run = TextRun {
                len: content.len(),
                font: style.font(),
                color: style.color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let runs = input
                .marked_range
                .as_ref()
                .and_then(|marked| {
                    let marked_start = marked.start.max(line_start);
                    let marked_end = marked.end.min(line_start + line_text.len());
                    (marked_start < marked_end).then(|| {
                        vec![
                            TextRun {
                                len: marked_start - line_start,
                                ..run.clone()
                            },
                            TextRun {
                                len: marked_end - marked_start,
                                underline: Some(UnderlineStyle {
                                    color: Some(style.color),
                                    thickness: px(1.),
                                    wavy: false,
                                }),
                                ..run.clone()
                            },
                            TextRun {
                                len: line_start + line_text.len() - marked_end,
                                ..run.clone()
                            },
                        ]
                    })
                })
                .unwrap_or_else(|| vec![run.clone()]);
            let line = window
                .text_system()
                .shape_line(content, font_size, &runs, None);
            lines.push((line_start, line));
            line_start += line_text.len() + 1;
        }
        let (cursor_line, cursor_offset) = lines
            .iter()
            .enumerate()
            .find_map(|(index, (start, line))| {
                let end = *start + line.len();
                (input.buffer.cursor() <= end || index + 1 == lines.len())
                    .then_some((index, input.buffer.cursor().saturating_sub(*start)))
            })
            .unwrap_or((0, 0));
        let cursor_x = lines[cursor_line].1.x_for_index(cursor_offset);
        let cursor = fill(
            Bounds::new(
                point(bounds.left() + cursor_x, bounds.top()),
                gpui::size(px(2.), bounds.bottom() - bounds.top()),
            ),
            gpui::blue(),
        );
        let line_height = window.line_height();
        let selection = input
            .buffer
            .selection()
            .into_iter()
            .flat_map(|range| {
                lines
                    .iter()
                    .enumerate()
                    .filter_map(move |(index, (start, line))| {
                        let line_start = *start;
                        let line_end = line_start + line.len();
                        let start_byte = range.start.max(line_start);
                        let end_byte = range.end.min(line_end);
                        (start_byte < end_byte).then(|| {
                            fill(
                                Bounds::from_corners(
                                    point(
                                        bounds.left() + line.x_for_index(start_byte - line_start),
                                        bounds.top() + line_height * index as f32,
                                    ),
                                    point(
                                        bounds.left() + line.x_for_index(end_byte - line_start),
                                        bounds.top() + line_height * (index + 1) as f32,
                                    ),
                                ),
                                gpui::rgba(0x3311ff30),
                            )
                        })
                    })
            })
            .collect();
        self.input.update(cx, |input, _| {
            input.last_layout = lines.get(cursor_line).map(|(_, line)| line.clone());
            input.last_lines = lines.clone();
            input.last_bounds = Some(bounds);
        });
        TextPrepaint {
            lines,
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if focus.is_focused(window) {
            for selection in &prepaint.selection {
                window.paint_quad(selection.clone());
            }
        }
        for (index, (_, line)) in prepaint.lines.iter().enumerate() {
            line.paint(
                point(
                    bounds.left(),
                    bounds.top() + window.line_height() * index as f32,
                ),
                window.line_height(),
                window,
                cx,
            )
            .expect("paint prompt");
        }
        if focus.is_focused(window) && prepaint.selection.is_empty() {
            window.paint_quad(prepaint.cursor.clone());
        }
    }
}
