use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler,
    FocusHandle, Focusable, GlobalElementId, IntoElement, KeyBinding, LayoutId, PaintQuad, Pixels,
    Render, ShapedLine, Style, TextRun, UTF16Selection, Window, fill, point, prelude::*, px,
    relative, rgb,
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
        self.move_to(
            next_grapheme(&self.text, self.cursor),
            selecting,
        );
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
        if line_start == 0 { return self.move_to(0, selecting); }
        let column = self.text[line_start..self.cursor].graphemes(true).count();
        let prior_end = line_start - 1;
        let prior_start = self.text[..prior_end].rfind('\n').map_or(0, |i| i + 1);
        self.move_to(grapheme_at_column(&self.text, prior_start, prior_end, column), selecting);
    }
    pub fn move_down(&mut self, selecting: bool) {
        let end = self.text[self.cursor..].find('\n').map_or(self.text.len(), |i| self.cursor + i);
        if end == self.text.len() { return self.move_to(end, selecting); }
        let start = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
        let column = self.text[start..self.cursor].graphemes(true).count();
        let next_start = end + 1;
        let next_end = self.text[next_start..].find('\n').map_or(self.text.len(), |i| next_start + i);
        self.move_to(grapheme_at_column(&self.text, next_start, next_end, column), selecting);
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
    s[..at].grapheme_indices(true).next_back().map(|(index, _)| index)
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
        e.move_right(false);
        e.move_right(false);
        e.move_down(false);
        assert_eq!(&e.text()[e.cursor()..], "\nz");
    }
}

pub struct TextInput {
    focus_handle: FocusHandle,
    buffer: EditorBuffer,
    history: Vec<String>,
    history_index: Option<usize>,
    disabled: bool,
    last_layout: Option<ShapedLine>,
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
            last_layout: None,
            last_bounds: None,
        }
    }

    pub fn content(&self) -> String {
        self.buffer.text().to_owned()
    }

    pub fn reset(&mut self) {
        self.buffer = EditorBuffer::default();
        self.history_index = None;
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

    fn move_left_selecting(&mut self, _: &MoveLeftSelecting, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_left(true);
        cx.notify();
    }

    fn move_right_selecting(&mut self, _: &MoveRightSelecting, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_right(true);
        cx.notify();
    }

    fn move_up_selecting(&mut self, _: &MoveUpSelecting, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_up(true);
        cx.notify();
    }

    fn move_down_selecting(&mut self, _: &MoveDownSelecting, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_down(true);
        cx.notify();
    }

    fn move_home_selecting(&mut self, _: &MoveHomeSelecting, _: &mut Window, cx: &mut Context<Self>) {
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

    fn history_next_action(
        &mut self,
        _: &HistoryNext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        let cursor = self.buffer.text()[..self.buffer.cursor()].encode_utf16().count();
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
        None
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}

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
        let range = range.map_or(self.buffer.cursor()..self.buffer.cursor(), |range| self.utf8_range(range));
        self.buffer.replace_range(range, text);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_text_in_range(range, text, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_layout.as_ref()?;
        let range = self.utf8_range(range);
        Some(Bounds::from_corners(
            point(bounds.left() + line.x_for_index(range.start), bounds.top()),
            point(bounds.left() + line.x_for_index(range.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let line = self.last_layout.as_ref()?;
        let index = line.index_for_x(point.x - bounds.left())?;
        Some(self.buffer.text()[..index].encode_utf16().count())
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
        let mut utf16 = 0;
        for (index, ch) in self.buffer.text().char_indices() {
            if utf16 >= offset {
                return index;
            }
            utf16 += ch.len_utf16();
        }
        self.buffer.text().len()
    }
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
    line: ShapedLine,
    cursor: PaintQuad,
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
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
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
        let content: gpui::SharedString = input.buffer.text().to_owned().into();
        let style = window.text_style();
        let run = TextRun {
            len: content.len(),
            font: style.font(),
            color: style.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window.text_system().shape_line(
            content,
            style.font_size.to_pixels(window.rem_size()),
            &[run],
            None,
        );
        let cursor_x = line.x_for_index(input.buffer.cursor());
        let cursor = fill(
            Bounds::new(
                point(bounds.left() + cursor_x, bounds.top()),
                gpui::size(px(2.), bounds.bottom() - bounds.top()),
            ),
            gpui::blue(),
        );
        self.input.update(cx, |input, _| {
            input.last_layout = Some(line.clone());
            input.last_bounds = Some(bounds);
        });
        TextPrepaint { line, cursor }
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
        prepaint
            .line
            .paint(bounds.origin, window.line_height(), window, cx)
            .expect("paint prompt");
        if focus.is_focused(window) {
            window.paint_quad(prepaint.cursor.clone());
        }
    }
}
