//! Repository operations panel view controller component.

use gpui::{App, IntoElement, ParentElement, Styled, Window, div};

use crate::components::{composer_pill, mac_button};
use crate::operations::OperationsModel;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationsTab {
    Status,
    Git,
    Changes,
}

#[allow(clippy::too_many_arguments)]
pub fn render_operations_panel(
    operations: &OperationsModel,
    operations_loading: bool,
    operations_error: Option<&str>,
    operations_tab: OperationsTab,
    theme: Theme,
    set_tab_status: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static,
    set_tab_git: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static,
    set_tab_changes: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static,
    refresh_btn: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static,
    open_file_btn: impl Fn(String) -> Box<dyn Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static>,
    stage_file_btn: impl Fn(
        String,
        &'static str,
    ) -> Box<dyn Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static>,
    keep_btn: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static,
    switch_branch_btn: impl Fn(
        String,
    ) -> Box<dyn Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static>,
    create_branch_btn: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static,
    acknowledge_btn: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let status = if operations_loading {
        "Loading…"
    } else {
        operations_error.unwrap_or("Ready")
    };
    let branch = if operations.branch.is_empty() {
        "(unknown)"
    } else {
        &operations.branch
    };
    let acknowledgement = operations
        .acknowledgement
        .map(|value| {
            if value {
                "acknowledged"
            } else {
                "not acknowledged"
            }
        })
        .unwrap_or("—");
    let active_tab = match operations_tab {
        OperationsTab::Status => "Status",
        OperationsTab::Git => "Git",
        OperationsTab::Changes => "Changes",
    };

    let mut panel = div()
        .w_full()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child("Repository"),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.tertiary_text)
                .child(format!("Status · {status}")),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.secondary_text)
                .child(format!("Branch: {branch}")),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.tertiary_text)
                .child(format!("Ack · {acknowledgement}")),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.tertiary_text)
                .child(format!("Tab · {active_tab}")),
        )
        .child(
            div()
                .flex()
                .gap_1()
                .child(composer_pill(
                    "Status",
                    "operations-status",
                    theme,
                    set_tab_status,
                ))
                .child(composer_pill("Git", "operations-git", theme, set_tab_git))
                .child(composer_pill(
                    "Changes",
                    "operations-changes",
                    theme,
                    set_tab_changes,
                ))
                .child(composer_pill(
                    "Refresh",
                    "operations-refresh",
                    theme,
                    refresh_btn,
                )),
        );

    panel = panel.children(operations.files.iter().map(|file| {
        let path = file.path.clone();
        let callback = open_file_btn(path.clone());
        div()
            .flex()
            .justify_between()
            .child(composer_pill(path, "operation-file", theme, callback))
            .child(if file.staged {
                "staged"
            } else if file.untracked {
                "untracked"
            } else {
                "modified"
            })
    }));

    if let Some(file) = &operations.selected {
        let stage_cb = stage_file_btn(file.path.clone(), "stage");
        let unstage_cb = stage_file_btn(file.path.clone(), "unstage");
        let revert_cb = stage_file_btn(file.path.clone(), "revert");
        let revision = crate::operations::content_revision(&file.content);
        let keep_label = if operations.is_kept(&file.path, &revision) {
            "Unkeep"
        } else {
            "Keep"
        };
        let mut details = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_color(theme.text).child(format!(
                "{}\n\nCONTENT\n{}\nDIFF\n{}",
                file.path, file.content, file.diff
            )))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(composer_pill("Stage", "op-stage", theme, stage_cb))
                    .child(composer_pill("Unstage", "op-unstage", theme, unstage_cb))
                    .child(mac_button(
                        "Revert (confirm)",
                        "op-revert",
                        false,
                        theme,
                        revert_cb,
                    ))
                    .child(composer_pill(keep_label, "op-keep", theme, keep_btn)),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.secondary_text)
                    .child("Branches"),
            );

        details = details.children(operations.branches.iter().map(|branch| {
            let name = branch.name.clone();
            let callback = switch_branch_btn(name.clone());
            composer_pill(name, "op-branch", theme, callback)
        }));

        details = details.child(composer_pill(
            "Create tau/gui-review",
            "op-create-branch",
            theme,
            create_branch_btn,
        ));
        details = details.child(composer_pill(
            "Acknowledge",
            "op-acknowledge",
            theme,
            acknowledge_btn,
        ));
        panel = panel.child(details);
    }

    panel
}
