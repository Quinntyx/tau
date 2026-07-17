use std::ops::Range;

use gpui::{
    App, Bounds, Context, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    FocusHandle, Focusable, GlobalElementId, IntoElement, KeyBinding, LayoutId, PaintQuad, Pixels,
    Render, ShapedLine, Style, TextRun, UTF16Selection, Window, fill, point, prelude::*, px,
    relative, rgb,
};

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
        if let Some(start) = self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
        {
            self.text.replace_range(start..self.cursor, "");
            self.cursor = start;
        }
    }
    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor < self.text.len() {
            let end = self.cursor + self.text[self.cursor..].chars().next().unwrap().len_utf8();
            self.text.replace_range(self.cursor..end, "");
        }
    }
    pub fn move_left(&mut self, selecting: bool) {
        self.move_to(
            self.text[..self.cursor]
                .char_indices()
                .next_back()
                .map_or(0, |(i, _)| i),
            selecting,
        );
    }
    pub fn move_right(&mut self, selecting: bool) {
        self.move_to(
            self.cursor
                + self.text[self.cursor..]
                    .chars()
                    .next()
                    .map_or(0, char::len_utf8),
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
    content: String,
    cursor: usize,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
}

impl TextInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: String::new(),
            cursor: 0,
            last_layout: None,
            last_bounds: None,
        }
    }

    pub fn content(&self) -> String {
        self.content.clone()
    }

    pub fn reset(&mut self) {
        self.content.clear();
        self.cursor = 0;
    }

    /// Replace the prompt text while preserving the input entity and focus.
    /// Picker views use this instead of reaching into the editor's storage.
    pub fn set_content(&mut self, content: impl Into<String>) {
        self.content = content.into();
        self.cursor = self.content.len();
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.cursor == 0 {
            return;
        }
        let start = self.content[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index);
        self.content.replace_range(start..self.cursor, "");
        self.cursor = start;
        cx.notify();
    }
}

gpui::actions!(tau_input, [Backspace]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("backspace", Backspace, None)]);
}

impl Render for TextInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::div()
            .key_context("TauPromptInput")
            .track_focus(&self.focus_handle(cx))
            .on_action(cx.listener(Self::backspace))
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
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let cursor = self.content[..self.cursor].encode_utf16().count();
        Some(UTF16Selection {
            range: cursor..cursor,
            reversed: false,
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
        let range = range.map_or(self.cursor..self.cursor, |range| self.utf8_range(range));
        self.content.replace_range(range.clone(), text);
        self.cursor = range.start + text.len();
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
        Some(self.content[..index].encode_utf16().count())
    }
}

impl TextInput {
    fn utf8_range(&self, range: Range<usize>) -> Range<usize> {
        let start = self.utf8_offset(range.start);
        let end = self.utf8_offset(range.end);
        start..end
    }

    fn utf16_range(&self, range: Range<usize>) -> Range<usize> {
        self.content[..range.start].encode_utf16().count()
            ..self.content[..range.end].encode_utf16().count()
    }

    fn utf8_offset(&self, offset: usize) -> usize {
        let mut utf16 = 0;
        for (index, ch) in self.content.char_indices() {
            if utf16 >= offset {
                return index;
            }
            utf16 += ch.len_utf16();
        }
        self.content.len()
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
        let content: gpui::SharedString = input.content.clone().into();
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
        let cursor_x = line.x_for_index(input.cursor);
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
