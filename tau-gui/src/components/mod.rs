//! Reusable UI component building blocks for tau-gui.

use crate::theme::Theme;
use gpui::{
    App, InteractiveElement, IntoElement, MouseButton, MouseUpEvent, ParentElement, Styled, Window,
    div, px, rgb,
};

pub fn sidebar_card(title: &str, value: &str, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .rounded_md()
        .bg(theme.surface)
        .border_1()
        .border_color(theme.separator)
        .child(
            div()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.tertiary_text)
                .child(title.to_string()),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.text)
                .child(value.to_string()),
        )
}

pub fn mac_button<T>(
    label: impl Into<String>,
    selector: &'static str,
    primary: bool,
    theme: Theme,
    callback: T,
) -> impl IntoElement
where
    T: Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
{
    div()
        .debug_selector(|| selector.into())
        .h(px(32.))
        .px_3()
        .flex()
        .items_center()
        .rounded_md()
        .bg(if primary {
            theme.accent
        } else {
            theme.elevated
        })
        .text_color(if primary { rgb(0xffffff) } else { theme.text })
        .hover(move |style| {
            style.bg(if primary {
                theme.accent_hover
            } else {
                theme.surface
            })
        })
        .cursor_pointer()
        .on_mouse_up(MouseButton::Left, callback)
        .child(label.into())
}

pub fn toast_button<T>(label: impl Into<String>, color: u32, callback: T) -> impl IntoElement
where
    T: Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
{
    div()
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(rgb(color))
        .hover(|style| style.opacity(0.9))
        .cursor_pointer()
        .on_mouse_up(MouseButton::Left, callback)
        .child(label.into())
}

pub fn testable_button<T>(
    label: impl Into<String>,
    color: u32,
    selector: &'static str,
    callback: T,
) -> impl IntoElement
where
    T: Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
{
    div()
        .debug_selector(|| selector.into())
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(rgb(color))
        .hover(|style| style.opacity(0.9))
        .cursor_pointer()
        .on_mouse_up(MouseButton::Left, callback)
        .child(label.into())
}

pub fn composer_pill<T>(
    label: impl Into<String>,
    selector: &'static str,
    theme: Theme,
    callback: T,
) -> impl IntoElement
where
    T: Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
{
    div()
        .debug_selector(|| selector.into())
        .h(px(28.))
        .px_3()
        .flex()
        .items_center()
        .rounded_md()
        .bg(theme.elevated)
        .text_color(theme.secondary_text)
        .text_xs()
        .hover(move |style| style.bg(theme.surface))
        .cursor_pointer()
        .on_mouse_up(MouseButton::Left, callback)
        .child(label.into())
}
