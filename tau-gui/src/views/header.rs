//! Header bar view controller component.

use crate::backend::DaemonStatus;
use crate::components::mac_button;
use crate::theme::Theme;
use gpui::{
    App, InteractiveElement, IntoElement, MouseButton, ParentElement, Styled, Window, div, px,
};

#[allow(clippy::too_many_arguments)]
pub fn render_header(
    current_model: impl Into<String>,
    sessions_open: bool,
    sidebar_visible: bool,
    follow_output: bool,
    daemon_status: DaemonStatus,
    theme: Theme,
    new_chat: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static,
    toggle_sessions: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static,
    toggle_sidebar: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static,
    toggle_follow: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static,
    open_account: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static,
    click_model: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let current_model = current_model.into();
    div()
        .flex()
        .items_center()
        .justify_between()
        .h(px(52.))
        .px_4()
        .border_b_1()
        .border_color(theme.separator)
        .bg(theme.toolbar)
        .child(
            div()
                .text_lg()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child("Tau"),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(mac_button(
                    "New Chat",
                    "new-chat-button",
                    true,
                    theme,
                    new_chat,
                ))
                .child(mac_button(
                    if sessions_open {
                        "Hide sessions"
                    } else {
                        "Sessions"
                    },
                    "sessions-button",
                    false,
                    theme,
                    toggle_sessions,
                ))
                .child(mac_button(
                    if sidebar_visible {
                        "Hide Inspector"
                    } else {
                        "Show Inspector"
                    },
                    "sidebar-button",
                    false,
                    theme,
                    toggle_sidebar,
                ))
                .child(mac_button(
                    if follow_output {
                        "Following"
                    } else {
                        "Follow output"
                    },
                    "follow-button",
                    false,
                    theme,
                    toggle_follow,
                ))
                .child(mac_button(
                    "Settings",
                    "settings-button",
                    false,
                    theme,
                    open_account,
                ))
                .child(
                    div()
                        .id("header-model-status")
                        .text_sm()
                        .text_color(theme.secondary_text)
                        .cursor_pointer()
                        .on_mouse_up(MouseButton::Left, click_model)
                        .child(format!(
                            "{}  ·  {}",
                            current_model,
                            match daemon_status {
                                DaemonStatus::Ready => "Ready",
                                DaemonStatus::Connecting
                                | DaemonStatus::Absent
                                | DaemonStatus::Spawning
                                | DaemonStatus::Negotiating
                                | DaemonStatus::Degraded => "Connecting",
                                DaemonStatus::Failed => "Request failed",
                                DaemonStatus::Incompatible => "Incompatible protocol",
                            }
                        )),
                ),
        )
}
