//! ChatGPT Account OAuth modal sheet view component.

use gpui::{App, IntoElement, ParentElement, Styled, Window, div, px};
use tau_proto::prelude::AuthState;

use crate::components::{mac_button, testable_button};
use crate::theme::Theme;
#[allow(clippy::too_many_arguments)]
pub fn render_account_sheet(
    auth_status: &AuthState,
    auth_loading: bool,
    auth_error: Option<&str>,
    theme: Theme,
    close_btn: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static,
    begin_btn: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static,
    open_url_btn: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static,
    copy_url_btn: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static,
    cancel_btn: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static,
    logout_btn: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let content = div()
        .w(px(520.))
        .max_w_full()
        .flex()
        .flex_col()
        .gap_4()
        .p_6()
        .rounded_xl()
        .border_1()
        .border_color(theme.separator)
        .bg(theme.elevated)
        .text_color(theme.text)
        .shadow_lg()
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_xl().child("ChatGPT account"))
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.secondary_text)
                                .child("Use your Plus or Pro subscription with Codex."),
                        ),
                )
                .child(mac_button(
                    "Close",
                    "account-close",
                    false,
                    theme,
                    close_btn,
                )),
        );

    let content = if let Some(error) = auth_error {
        content.child(
            div()
                .p_3()
                .rounded_md()
                .bg(theme.error_surface)
                .text_color(theme.error_text)
                .child(error.to_string()),
        )
    } else {
        content
    };

    let content = match auth_status {
        AuthState::SignedOut => content
            .child(
                div()
                    .text_sm()
                    .text_color(theme.secondary_text)
                    .child("Tau will open a secure browser authorization page. Your password is never visible to Tau."),
            )
            .child(mac_button(
                if auth_loading { "Connecting..." } else { "Connect ChatGPT" },
                "account-connect",
                true,
                theme,
                begin_btn,
            )),
        AuthState::Pending { authorize_url } => content
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_sm().child("Authorization is waiting in your browser."))
                    .child(
                        div()
                            .p_3()
                            .rounded_md()
                            .bg(theme.canvas)
                            .text_xs()
                            .text_color(theme.accent)
                            .child(authorize_url.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(mac_button(
                        "Open Browser",
                        "account-open-browser",
                        true,
                        theme,
                        open_url_btn,
                    ))
                    .child(mac_button(
                        "Copy Link",
                        "account-copy-link",
                        false,
                        theme,
                        copy_url_btn,
                    ))
                    .child(mac_button(
                        "Cancel",
                        "account-cancel",
                        false,
                        theme,
                        cancel_btn,
                    )),
            ),
        AuthState::SignedIn { email, account_id } => content
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .rounded_md()
                    .bg(theme.success_surface)
                    .text_color(theme.success_text)
                    .child("Connected")
                    .children(email.iter().cloned().map(|value| div().text_sm().child(value)))
                    .children(account_id.iter().cloned().map(|value| {
                        div()
                            .text_xs()
                            .text_color(theme.secondary_text)
                            .child(value)
                    })),
            )
            .child(testable_button(
                "Sign Out",
                0x8b3038,
                "account-logout",
                logout_btn,
            )),
        AuthState::Failed { message } => content
            .child(
                div()
                    .p_3()
                    .rounded_md()
                    .bg(theme.error_surface)
                    .text_color(theme.error_text)
                    .child(message.clone()),
            )
            .child(mac_button(
                "Try Again",
                "account-retry",
                true,
                theme,
                begin_btn,
            )),
    };

    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::rgba(0x00000080))
        .child(content)
}
