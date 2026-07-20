use ratatui::{Terminal, backend::TestBackend};
use tau_proto::git::{GitFileResult, GitFileStatus};
use tau_tui::{
    AppState, components,
    operations::OperationsState,
    projects::{ProjectAction, ProjectState},
    sessions::{ProjectId, SessionEntry, SessionId},
};

fn snapshot(state: &AppState, projects: &ProjectState) -> String {
    let mut terminal = Terminal::new(TestBackend::new(180, 36)).unwrap();
    terminal
        .draw(|frame| components::render_with_projects(frame, state, projects))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer.cell((x, y)).unwrap().symbol())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_full_shell_snapshot(output: &str, expected: &[&str]) {
    // Keep this acceptance test independent of a real terminal.  In
    // particular, do not snapshot ANSI escapes or rely on terminal size
    // discovery: TestBackend is the same non-interactive path used by CI.
    assert_eq!(
        output.split('\n').count(),
        36,
        "full shell frame changed:\n{output}"
    );
    assert!(
        !output.contains('\u{1b}'),
        "raw ANSI escape in shell snapshot"
    );
    for text in expected {
        assert!(output.contains(text), "snapshot missing {text:?}\n{output}");
    }
}

#[test]
fn complete_shell_composition_snapshot() {
    let mut state = AppState::with_ids("session-42", "/workspace/alpha");
    state.input = "Review the staged change".into();
    state.cursor = state.input.len();
    state.transcript = vec![
        "Human: inspect the staged change".into(),
        "Assistant: I found one file.".into(),
    ];
    state.sessions.open = true;
    state.sessions.project = Some(ProjectId::new("/workspace/alpha"));
    state.sessions.sessions.push(SessionEntry {
        id: SessionId::new("session-42"),
        project_id: ProjectId::new("/workspace/alpha"),
        title: "Review staged change".into(),
        updated_at: 2,
        archived: false,
    });
    state.operations = OperationsState::new("/workspace/alpha");
    state.operations.branch = "feature/acceptance".into();
    state.operations.files = vec![GitFileStatus {
        path: "src/main.rs".into(),
        staged: true,
        modified: true,
        untracked: false,
    }];
    state.operations.content = Some(GitFileResult {
        path: "src/main.rs".into(),
        content: "fn main() {}".into(),
        diff: "+accepted".into(),
    });
    state.operations_focused = true;

    let mut projects = ProjectState::default();
    projects
        .apply(ProjectAction::Register {
            name: "alpha".into(),
            root: "/workspace/alpha".into(),
        })
        .unwrap();
    let output = snapshot(&state, &projects);
    assert_full_shell_snapshot(
        &output,
        &[
            "Projects",
            "alpha",
            "Sessions ·",
            "Ctrl-S close",
            "Review staged change",
            "Human: inspect",
            "prompt (Shift+Enter newline)",
            "Review the staged change",
            "Operations [fe",
            "src/main.rs",
            "Status",
            "Git",
            "Changes",
            "● connected",
            "open  [R] refresh",
            "Selected project: alpha",
        ],
    );
}

#[test]
fn selector_and_status_snapshot_is_deterministic() {
    let mut state = AppState::with_ids("session-select", "/workspace/alpha");
    state.picker = tau_tui::state::Picker::Models;
    state.connection = tau_tui::state::Connection::Reconnecting;
    state.operations_loading = true;
    state.operations_error = Some("daemon busy".into());
    let mut projects = ProjectState::default();
    projects
        .apply(ProjectAction::Register {
            name: "alpha".into(),
            root: "/workspace/alpha".into(),
        })
        .unwrap();
    let output = snapshot(&state, &projects);
    assert_full_shell_snapshot(
        &output,
        &[
            "Select model (favorites",
            "★ openai/gpt-4o",
            "anthropic/claude-sonnet",
            "◌ reconnecting",
            "Loading: yes",
            "Error: daemon busy",
        ],
    );
}
