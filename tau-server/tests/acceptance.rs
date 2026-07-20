//! M13 acceptance fixtures.  These are deliberately small scripted doubles:
//! they exercise the public seams without reimplementing an agent, MCP, LSP,
//! or daemon.

mod fixtures {
    use std::{path::PathBuf, process::Command, sync::Arc};

    use anyhow::Result;
    use tau_core::git::{GitTopology, GitWorkspace};
    use tau_proto::prelude::*;
    use tempfile::{TempDir, tempdir};
    use tokio::net::UnixListener;

    pub struct ScriptedProvider {
        pub turns: Vec<Vec<&'static str>>,
        pub calls: Vec<String>,
    }

    impl ScriptedProvider {
        pub fn new(turns: Vec<Vec<&'static str>>) -> Self {
            Self {
                turns,
                calls: vec![],
            }
        }
        pub fn complete(&mut self, prompt: &str) -> Vec<&'static str> {
            self.calls.push(prompt.to_owned());
            self.turns.first().cloned().unwrap_or_default()
        }
    }

    pub struct ServerFixture {
        pub _dir: TempDir,
        pub socket: PathBuf,
        pub task: tokio::task::JoinHandle<()>,
    }

    impl ServerFixture {
        pub async fn start() -> Result<Self> {
            Self::start_with_state(tau_server::AppState::default()).await
        }

        pub async fn start_with_state(state: tau_server::AppState) -> Result<Self> {
            let dir = tempdir()?;
            let socket = dir.path().join("tau.sock");
            let listener = UnixListener::bind(&socket)?;
            let app = tau_server::router(state);
            let task = tokio::spawn(async move {
                let _ = axum::serve(listener, app.into_make_service()).await;
            });
            Ok(Self {
                _dir: dir,
                socket,
                task,
            })
        }
    }

    /// A real WebSocket client, kept behind the same fixture lifetime as the
    /// daemon so tests cannot accidentally connect to a user's daemon.
    pub async fn client(fixture: &ServerFixture) -> Result<tau_client::Client> {
        tau_client::Client::connect(&fixture.socket).await
    }

    pub struct StdioScript {
        pub _dir: TempDir,
        pub command: String,
        pub args: Vec<String>,
    }

    impl StdioScript {
        pub fn new() -> Result<Self> {
            let dir = tempdir()?;
            let script = dir.path().join("rpc.py");
            std::fs::write(&script, SCRIPT)?;
            Ok(Self {
                _dir: dir,
                command: "python3".into(),
                args: vec![script.to_string_lossy().into_owned()],
            })
        }
        pub fn mcp_config(&self) -> tau_core::integrations::McpServerConfig {
            tau_core::integrations::McpServerConfig {
                command: self.command.clone(),
                args: self.args.clone(),
                timeout_ms: 2_000,
                env: std::collections::BTreeMap::new(),
                cwd: None,
                max_restarts: 1,
            }
        }
        pub fn lsp_config(&self, root: PathBuf) -> tau_core::integrations::LspServerConfig {
            tau_core::integrations::LspServerConfig {
                command: self.command.clone(),
                args: self.args.clone(),
                root,
                language_id: "rust".into(),
                timeout_ms: 2_000,
            }
        }
    }

    const SCRIPT: &str = r#"import json,sys
def send(value, line_mode):
    body=json.dumps(value).encode()
    if line_mode:
        sys.stdout.buffer.write(body+b'\n')
    else:
        sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n'%len(body)).encode()+body)
    sys.stdout.buffer.flush()
while True:
    line=sys.stdin.buffer.readline()
    if not line: break
    line_mode=line.lstrip().startswith(b'{')
    if line_mode:
        body=line
    else:
        headers={}
        while line not in (b'\r\n',b'\n',b''):
            k,v=line.decode().split(':',1); headers[k.lower()]=v.strip()
            line=sys.stdin.buffer.readline()
        if not line: break
        body=sys.stdin.buffer.read(int(headers['content-length']))
    req=json.loads(body); method=req.get('method',''); result={}
    if method == 'initialize': result={'protocolVersion':'2025-06-18','capabilities':{'tools':{},'prompts':{}},'serverInfo':{'name':'tau-fixture','version':'1'}}
    elif method == 'tools/list': result={'tools':[{'name':'echo','description':'fixture','inputSchema':{'type':'object'}}]}
    elif method == 'prompts/list': result={'prompts':[{'name':'plan','description':'fixture'}]}
    elif method == 'tools/call': result={'content':[{'type':'text','text':'ok'}]}
    elif method.startswith('textDocument/'):
        result=[{'uri':'file:///fixture.rs','range':{'start':{'line':0,'character':0},'end':{'line':0,'character':1}}}]
    if 'id' in req:
        response={'jsonrpc':'2.0','id':req.get('id'),'result':result}
        send(response, line_mode)
"#;

    pub struct GitFixture {
        pub _dir: TempDir,
        pub root: PathBuf,
        pub workspace: GitWorkspace,
    }

    impl GitFixture {
        pub fn new() -> Result<Self> {
            let dir = tempdir()?;
            let workspace = GitWorkspace::initialize(dir.path(), GitTopology::Direct, vec![])?;
            git(
                dir.path(),
                &["config", "user.email", "fixture@example.invalid"],
            )?;
            git(dir.path(), &["config", "user.name", "fixture"])?;
            std::fs::write(dir.path().join(".gitignore"), ".tau/\n")?;
            std::fs::write(dir.path().join("tracked.txt"), "base\n")?;
            git(dir.path(), &["add", ".gitignore", "tracked.txt"])?;
            git(dir.path(), &["commit", "-m", "base"])?;
            Ok(Self {
                _dir: dir,
                root: workspace.manifest.root.clone(),
                workspace,
            })
        }
    }

    fn git(path: &std::path::Path, args: &[&str]) -> Result<()> {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    pub fn protocol_fixture() -> (ProtocolNegotiateParams, ProtocolNegotiateResult) {
        let capabilities = vec![
            Capability::TurnStreaming,
            Capability::EventReplay,
            Capability::Idempotency,
        ];
        (
            ProtocolNegotiateParams {
                version: ProtocolVersion { major: 1, minor: 0 },
                capabilities: capabilities.clone(),
            },
            ProtocolNegotiateResult {
                version: ProtocolVersion { major: 1, minor: 2 },
                capabilities,
            },
        )
    }

    pub fn _arc<T>(value: T) -> Arc<T> {
        Arc::new(value)
    }
}

use anyhow::{Context, Result};
use futures::StreamExt;
use tau_proto::prelude::*;

async fn run_typed_turn(client: &tau_client::Client, params: TurnStartParams) -> Result<String> {
    let mut events = client.events();
    let mut admission = client.turn_start(params).await?;
    let started = loop {
        match admission
            .next()
            .await
            .context("turn admission stream closed")??
        {
            tau_client::TurnStreamEvent::Complete(result) => break result,
            tau_client::TurnStreamEvent::Event(_) => {}
        }
    };
    let turn_id = started.turn_id;
    tokio::time::timeout(std::time::Duration::from_secs(10), async move {
        let mut text = String::new();
        loop {
            let event = events.next().await.context("turn event stream closed")??;
            match event.event {
                TurnEvent::TextDelta {
                    turn_id: event_turn,
                    text: delta,
                } if event_turn == turn_id => text.push_str(&delta),
                TurnEvent::TurnCompleted {
                    turn_id: event_turn,
                    ..
                } if event_turn == turn_id => return Ok::<String, anyhow::Error>(text),
                TurnEvent::TurnFailed {
                    turn_id: event_turn,
                    message,
                } if event_turn == turn_id => anyhow::bail!(message),
                TurnEvent::TurnCancelled {
                    turn_id: event_turn,
                } if event_turn == turn_id => anyhow::bail!("turn cancelled"),
                _ => {}
            }
        }
    })
    .await
    .context("typed turn timed out")?
}

#[tokio::test]
async fn websocket_protocol_negotiation_and_scripted_provider_round_trip() -> Result<()> {
    let (request, latest) = fixtures::protocol_fixture();
    let encoded = serde_json::to_value(&request)?;
    let decoded: ProtocolNegotiateParams = serde_json::from_value(encoded)?;
    assert_eq!(decoded.version.major, 1);
    assert!(latest.version.minor >= decoded.version.minor);
    let mut provider = fixtures::ScriptedProvider::new(vec![vec!["plan", "done"]]);
    assert_eq!(provider.complete("plan?"), vec!["plan", "done"]);
    assert_eq!(provider.calls, ["plan?"]);

    let fixture = fixtures::ServerFixture::start().await?;
    let client = fixtures::client(&fixture).await?;
    assert_eq!(client.ping().await?, "pong");
    let negotiated = client.negotiate(request.clone()).await?;
    assert_eq!(negotiated.version.major, 1);
    assert!(negotiated.capabilities.contains(&Capability::EventReplay));
    let mismatch = client
        .call(
            METHOD_PROTOCOL_NEGOTIATE,
            Some(serde_json::json!({
                "version": {"major": 99, "minor": 0},
                "capabilities": []
            })),
        )
        .await;
    assert!(
        mismatch.is_err(),
        "server must reject incompatible protocol majors"
    );
    fixture.task.abort();
    Ok(())
}

#[tokio::test]
async fn typed_git_operations_are_contained_and_safe() -> Result<()> {
    let git = fixtures::GitFixture::new()?;
    let fixture = fixtures::ServerFixture::start().await?;
    let client = fixtures::client(&fixture).await?;
    let project = client
        .project_create(ProjectCreateParams {
            name: "git fixture".into(),
            root: git.root.to_string_lossy().into_owned(),
        })
        .await?
        .project
        .id;

    assert!(
        client
            .git_status(GitStatusParams {
                project: "missing-project".into(),
            })
            .await
            .is_err()
    );
    client
        .project_unregister(ProjectIdParams {
            project_id: project.clone(),
        })
        .await?;
    assert!(
        client
            .git_status(GitStatusParams {
                project: project.clone(),
            })
            .await
            .is_err()
    );
    client
        .project_reactivate(ProjectIdParams {
            project_id: project.clone(),
        })
        .await?;

    let status = client
        .git_status(GitStatusParams {
            project: project.clone(),
        })
        .await?;
    assert_eq!(status.branch, "master");
    assert!(status.files.is_empty());

    std::fs::write(git.root.join("tracked.txt"), "changed\n")?;
    let file = client
        .git_file(GitFileParams {
            project: project.clone(),
            path: "tracked.txt".into(),
        })
        .await?;
    assert_eq!(file.content, "changed\n");
    assert!(file.diff.contains("-base"));
    assert!(
        client
            .git_stage(GitPathParams {
                project: project.clone(),
                path: "tracked.txt".into(),
            })
            .await?
            .acknowledged
    );
    assert!(
        client
            .git_status(GitStatusParams {
                project: project.clone(),
            })
            .await?
            .files[0]
            .staged
    );
    client
        .git_unstage(GitPathParams {
            project: project.clone(),
            path: "tracked.txt".into(),
        })
        .await?;

    assert!(
        client
            .git_revert(GitRevertParams {
                project: project.clone(),
                path: "tracked.txt".into(),
                confirmed: false,
            })
            .await
            .is_err()
    );
    client
        .git_revert(GitRevertParams {
            project: project.clone(),
            path: "tracked.txt".into(),
            confirmed: true,
        })
        .await?;
    assert_eq!(
        std::fs::read_to_string(git.root.join("tracked.txt"))?,
        "base\n"
    );

    client
        .git_branch_create(GitBranchCreateParams {
            project: project.clone(),
            name: "feature".into(),
        })
        .await?;
    assert!(
        client
            .git_branches(GitBranchesParams {
                project: project.clone(),
            })
            .await?
            .branches
            .iter()
            .any(|branch| branch.name == "feature")
    );
    std::fs::write(git.root.join("tracked.txt"), "dirty\n")?;
    assert!(
        client
            .git_branch_switch(GitBranchSwitchParams {
                project: project.clone(),
                name: "feature".into(),
            })
            .await
            .is_err()
    );
    client
        .git_revert(GitRevertParams {
            project: project.clone(),
            path: "tracked.txt".into(),
            confirmed: true,
        })
        .await?;
    client
        .git_branch_switch(GitBranchSwitchParams {
            project: project.clone(),
            name: "feature".into(),
        })
        .await?;

    assert!(
        client
            .git_file(GitFileParams {
                project: project.clone(),
                path: "../outside".into(),
            })
            .await
            .is_err()
    );
    let first = client
        .git_ack(GitAckParams {
            project: project.clone(),
            operation: "revert".into(),
            acknowledged: false,
        })
        .await?;
    let second = client
        .git_ack(GitAckParams {
            project,
            operation: "revert".into(),
            acknowledged: true,
        })
        .await?;
    assert_eq!(first, second);
    fixture.task.abort();
    Ok(())
}

#[tokio::test]
async fn mcp_and_lsp_scripted_processes_support_discovery_restart_and_queries() -> Result<()> {
    let script = fixtures::StdioScript::new()?;
    let mut mcp = tau_core::integrations::McpClient::connect(script.mcp_config()).await?;
    assert_eq!(mcp.tools().await?[0].name, "echo");
    assert_eq!(mcp.prompts().await?[0].name, "plan");
    assert_eq!(
        mcp.call_tool("echo", serde_json::json!({})).await?["content"][0]["text"],
        "ok"
    );
    mcp.restart().await?;
    assert_eq!(mcp.tools().await?.len(), 1);

    let root = tempfile::tempdir()?;
    let mut lsp =
        tau_core::integrations::LspClient::connect(script.lsp_config(root.path().to_owned()))
            .await?;
    let locations = lsp
        .definition(
            "file:///fixture.rs",
            tau_core::integrations::LspPosition {
                line: 0,
                character: 0,
            },
        )
        .await?;
    assert_eq!(locations.len(), 1);
    lsp.record_diagnostics("file:///fixture.rs", vec![]);
    assert!(lsp.diagnostics("file:///fixture.rs").await.is_empty());
    lsp.restart().await?;
    Ok(())
}

#[test]
fn git_fixture_keeps_unrelated_dirty_work_and_requires_explicit_commit() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let workspace = tau_core::git::GitWorkspace::initialize(
        dir.path(),
        tau_core::git::GitTopology::Direct,
        vec![],
    )?;
    std::process::Command::new("git")
        .args([
            "-C",
            dir.path().to_str().unwrap(),
            "config",
            "user.email",
            "fixture@example.invalid",
        ])
        .status()?;
    std::process::Command::new("git")
        .args([
            "-C",
            dir.path().to_str().unwrap(),
            "config",
            "user.name",
            "fixture",
        ])
        .status()?;
    std::fs::write(dir.path().join(".gitignore"), ".tau/\n")?;
    std::fs::write(dir.path().join("tracked.txt"), "base\n")?;
    std::fs::write(dir.path().join("unrelated.txt"), "dirty\n")?;
    std::process::Command::new("git")
        .args([
            "-C",
            dir.path().to_str().unwrap(),
            "add",
            ".gitignore",
            "tracked.txt",
        ])
        .status()?;
    std::process::Command::new("git")
        .args(["-C", dir.path().to_str().unwrap(), "commit", "-m", "base"])
        .status()?;
    std::fs::write(dir.path().join("tracked.txt"), "changed\n")?;
    let hash = workspace.commit(dir.path(), "explicit", &["tracked.txt".into()])?;
    assert!(!hash.is_empty());
    let status = std::process::Command::new("git")
        .args(["-C", dir.path().to_str().unwrap(), "status", "--porcelain"])
        .output()?;
    assert_eq!(
        String::from_utf8_lossy(&status.stdout).trim(),
        "?? unrelated.txt"
    );
    Ok(())
}

#[test]
fn reusable_git_fixture_creates_a_managed_worktree() -> Result<()> {
    let fixture = fixtures::GitFixture::new()?;
    let worktree = fixture
        .workspace
        .managed_worktree("scripted/model", &fixture.root)?;
    assert!(worktree.path.join("tracked.txt").is_file());
    assert!(worktree.branch.starts_with("tau/"));
    Ok(())
}

#[test]
fn snapshot_and_daemon_fixtures_cover_traversal_replay_and_ownership() -> Result<()> {
    let root = tempfile::tempdir()?;
    let file = root.path().join("safe.txt");
    std::fs::write(&file, "safe")?;
    let store = tau_core::tools::SnapshotStore::for_cwd(root.path());
    let capture = store.capture_paths(std::slice::from_ref(&file))?;
    assert_eq!(store.restore(&capture.id)?, 1);
    assert!(store.restore("../escape").is_err());

    let log = tau_server::runtime::EventLog::new(4);
    assert_eq!(log.append("started"), 1);
    assert_eq!(log.append("done"), 2);
    assert_eq!(log.replay_since(1), vec![(2, "done")]);
    let registry = tau_server::runtime::ConnectionRegistry::new();
    let owner = registry.attach();
    assert!(registry.is_attached(owner));
    assert!(registry.detach(owner));
    assert!(!registry.is_attached(owner));
    Ok(())
}

#[test]
fn production_plan_gate_migration_and_git_preview_are_not_fixture_state() -> Result<()> {
    let mut plan = tau_core::plan::Plan::new("acceptance", "contract");
    let step = plan.add_step("mutate");
    assert!(tau_core::plan::allows_tool(&plan, true, "edit").is_err());
    assert!(tau_core::plan::allows_tool(&plan, false, "read").is_ok());
    assert!(plan.airtight_step(step));
    assert!(tau_core::plan::allows_tool(&plan, false, "bash").is_ok());
    plan.revoke_airtight();
    assert!(tau_core::plan::allows_tool(&plan, true, "write").is_err());

    let db = tau_core::db::Db::open_in_memory()?;
    let project = db.create_project("acceptance", "/tmp/acceptance")?;
    let session = db.create_session(&project.id)?;
    let event = tau_proto::turn::TurnEvent::TextDelta {
        turn_id: "turn".into(),
        text: "replayed".into(),
    };
    db.append_event(&session.id, &event, None)?;
    assert_eq!(db.replay_events(&session.id, 0, None)?.len(), 1);

    let git = fixtures::GitFixture::new()?;
    let worktree = git
        .workspace
        .managed_worktree("acceptance/model", &git.root)?;
    std::fs::write(worktree.path.join("preview.txt"), "preview\n")?;
    let hash = git
        .workspace
        .commit(&worktree.path, "preview", &["preview.txt".into()])?;
    assert!(!hash.is_empty());
    let preview = git
        .workspace
        .preview_integration(&git.root, &worktree.branch, "master")?;
    assert!(preview.commits.iter().any(|commit| commit == &hash));
    assert!(!preview.conflicts);
    Ok(())
}

#[test]
fn snapshot_restore_rejects_manifest_escape_and_compaction_preserves_summary() -> Result<()> {
    let root = tempfile::tempdir()?;
    let file = root.path().join("safe.txt");
    std::fs::write(&file, "before")?;
    let store = tau_core::tools::SnapshotStore::for_cwd(root.path());
    let capture = store.capture_paths(std::slice::from_ref(&file))?;
    let manifest = std::fs::read_to_string(&capture.manifest)?;
    std::fs::write(
        &capture.manifest,
        manifest.replace("safe.txt", "../escaped.txt"),
    )?;
    assert!(store.restore(&capture.id).is_err());

    let mut context = tau_core::context::ContextAssembler::new(1);
    context.set_plan_context("# Plan");
    context.set_provider_metadata("production", "compactor");
    context.push("user", "long input");
    let old = context.compact("durable summary");
    assert_eq!(old.number, 0);
    assert_eq!(context.epoch().number, 1);
    assert!(
        context.epoch().messages[0]
            .content
            .contains("durable summary")
    );
    assert_eq!(context.epoch().provider.as_deref(), Some("production"));
    Ok(())
}

#[test]
fn gui_and_tui_state_fixtures_cover_permission_diff_tiers_and_cancel() {
    let mut tui = tau_tui::AppState::default();
    tau_tui::reduce(&mut tui, tau_tui::Action::Tier(1));
    tau_tui::reduce(&mut tui, tau_tui::Action::ToggleAutonomy);
    tau_tui::reduce(&mut tui, tau_tui::Action::Cancel);
    assert_eq!(tui.task_tier, 2);
    assert!(tui.autonomous && tui.cancelling);

    let mut gui = tau_gui::chat::ChatState::default();
    assert!(gui.reduce(tau_gui::chat::ChatAction::Submit("hello".into())));
    assert_eq!(gui.cards.len(), 2);
}

/// Mandatory M13 closure chain. The only double here is Rig's scripted model
/// at the provider boundary; negotiation, persistence, completion and replay
/// are driven through the real server/client APIs. Plan/permission/file/git
/// stages use their real tau-core APIs because no corresponding JSON-RPC
/// methods exist yet; this test intentionally does not claim those absent
/// server surfaces are covered.
#[tokio::test]
async fn mandatory_production_agent_workflow_closure() -> Result<()> {
    use rig_core::test_utils::{MockCompletionModel, MockStreamEvent};

    let scripted = MockCompletionModel::from_stream_turns([
        [
            MockStreamEvent::text("first model result"),
            MockStreamEvent::final_response_with_total_tokens(3),
        ],
        [
            MockStreamEvent::text("second model result"),
            MockStreamEvent::final_response_with_total_tokens(4),
        ],
    ]);
    let state = tau_server::AppState::default()
        .with_provider(tau_core::provider::Provider::scripted(scripted));
    let project = state
        .db()
        .create_project("m13-production", "/tmp/m13-production")?;
    let session = state.db().create_session(&project.id)?;
    let qa = state
        .db()
        .record_qa(&session.id, "May the actor edit?", "Yes, only tracked.txt")?;
    let qa = state
        .db()
        .get_qa_records(&session.id)?
        .into_iter()
        .find(|record| record.id == qa.id)
        .expect("recorded Q&A must be reloadable before plan citation");
    let mut plan = tau_core::plan::Plan::new("closure", "do the requested mutation");
    let step = plan.add_step("authorized mutation");
    assert!(plan.attach_qa(step, qa.id.clone(), tau_core::plan::PlanAuthority::Human));
    assert!(
        plan.render_markdown_with_qa(std::slice::from_ref(&qa))
            .contains("only tracked.txt")
    );
    assert!(tau_core::plan::allows_tool(&plan, false, "write").is_err());
    assert!(plan.airtight_step(step));
    assert!(tau_core::plan::allows_tool(&plan, false, "write").is_ok());

    let broker = tau_core::permissions::PermissionBroker::default();
    let request = broker
        .request("m13", "write", serde_json::json!({"path":"tracked.txt"}))
        .await;
    assert!(
        broker
            .reply(&request.id, tau_core::permissions::PermissionReply::Allow)
            .await
    );
    assert_eq!(
        broker.wait(&request.id).await,
        Some(tau_core::permissions::PermissionReply::Allow)
    );

    let git = fixtures::GitFixture::new()?;
    let worktree = git.workspace.managed_worktree("closure/model", &git.root)?;
    let root = worktree.path.clone();
    let file = root.join("tracked.txt");
    std::fs::write(&file, "before\n")?;
    let mut transaction =
        tau_core::tools::SnapshotTransaction::begin(&root, std::slice::from_ref(&file))?;
    std::fs::write(&file, "after\n")?;
    let diff = transaction.diff()?;
    assert_eq!(diff[0].hunks.len(), 1);
    assert_eq!(
        transaction.apply(
            &[(file.clone(), tau_core::tools::FileDecision::Accept)],
            false
        )?,
        1
    );
    let commit = git.workspace.commit(
        &root,
        "accept authorized mutation",
        &[std::path::PathBuf::from("tracked.txt")],
    )?;
    let preview = git
        .workspace
        .preview_integration(&git.root, &worktree.branch, "master")?;
    assert!(preview.commits.iter().any(|candidate| candidate == &commit));
    assert!(
        !preview.conflicts,
        "integration preview must be conflict-free"
    );

    let fixture = fixtures::ServerFixture::start_with_state(state.clone()).await?;
    let client = fixtures::client(&fixture).await?;
    let negotiated = client.negotiate(fixtures::protocol_fixture().0).await?;
    assert_eq!(negotiated.version.major, 1);

    let first_text = run_typed_turn(
        &client,
        TurnStartParams {
            project_id: project.id.clone(),
            model: "scripted/model".into(),
            prompt: "run the authorized mutation".into(),
            session_id: Some(session.id.clone()),
            cwd: Some(root.to_string_lossy().into_owned()),
            idempotency_key: IdempotencyKey::new("m13-first-model-result"),
            agent: Some("code".into()),
            task_tier: Some(1),
            autonomous: Some(false),
            action: Some(RequestAction::Submit),
        },
    )
    .await?;
    assert_eq!(first_text, "first model result");
    let messages = state.db().get_messages(&session.id)?;
    assert!(
        messages.iter().any(|message| {
            message.role == "user"
            && message.blocks.iter().any(|block| matches!(
                block,
                tau_core::db::ContentBlock::Text { text } if text == "run the authorized mutation"
            ))
        }),
        "server completion must persist the user prompt"
    );
    assert!(
        messages.iter().any(|message| {
            message.role == "assistant"
                && message.blocks.iter().any(|block| {
                    matches!(
                        block,
                        tau_core::db::ContentBlock::Text { text } if text == "first model result"
                    )
                })
        }),
        "server completion must persist the model result"
    );

    let second_text = run_typed_turn(
        &client,
        TurnStartParams {
            project_id: project.id.clone(),
            model: "scripted/model".into(),
            prompt: "continue after review".into(),
            session_id: Some(session.id.clone()),
            cwd: Some(root.to_string_lossy().into_owned()),
            idempotency_key: IdempotencyKey::new("m13-second-model-result"),
            agent: Some("code".into()),
            task_tier: Some(1),
            autonomous: Some(false),
            action: Some(RequestAction::Submit),
        },
    )
    .await?;
    assert_eq!(second_text, "second model result");

    // Replay must be produced by the typed server API, not by appending a
    // fixture event directly to the database.
    let _turn = client
        .turn_start(tau_proto::turn::TurnStartParams {
            project_id: project.id.clone(),
            model: "scripted/model".into(),
            prompt: "replay the authorized mutation".into(),
            session_id: Some(session.id.clone()),
            cwd: Some(root.to_string_lossy().into_owned()),
            idempotency_key: tau_proto::turn::IdempotencyKey::new("m13-replay"),
            agent: Some("code".into()),
            task_tier: Some(1),
            autonomous: Some(false),
            action: Some(tau_proto::turn::RequestAction::Replay),
        })
        .await?;
    drop(client); // explicit disconnect before replay
    let replay_client = fixtures::client(&fixture).await?;
    replay_client
        .negotiate_checked(ProtocolNegotiateParams {
            version: ProtocolVersion { major: 1, minor: 0 },
            capabilities: vec![Capability::EventReplay],
        })
        .await?;
    let replay = replay_client
        .turn_replay(tau_proto::turn::TurnReplayParams {
            session_id: session.id.clone(),
            after_sequence: 0,
            limit: None,
        })
        .await?;
    assert!(
        replay
            .events
            .iter()
            .any(|event| { matches!(event.event, tau_proto::turn::TurnEvent::TurnStarted { .. }) }),
        "replay must contain the server-created turn event"
    );

    let plan_markdown = plan.render_markdown_with_qa(std::slice::from_ref(&qa));
    let mut context = tau_core::context::ContextAssembler::new(1);
    context.set_plan_context(&plan_markdown);
    context.set_provider_metadata("scripted", "model");
    context.push("user", "compact this");
    let compacted = context.compact("summary with plan reinjection");
    assert!(
        context
            .epoch()
            .plan_context
            .as_deref()
            .unwrap_or_default()
            .contains("closure")
    );
    assert!(context.epoch().messages[0].content.contains("summary"));
    let mut epoch = tau_core::db::ContextEpochRecord::new(
        &session.id,
        context.epoch().number as i64,
        context.epoch().messages[0].content.clone(),
        "manual_acceptance",
    );
    epoch.plan_context = compacted.plan_context;
    epoch.provider = compacted.provider;
    epoch.model = compacted.compaction_model;
    epoch.retry_marker = compacted.retry_marker;
    state.db().append_context_epoch(&epoch)?;
    let persisted = state
        .db()
        .latest_context_epoch(&session.id)?
        .expect("compaction epoch must be durable");
    assert!(
        persisted
            .plan_context
            .as_deref()
            .unwrap_or_default()
            .contains("closure")
    );
    fixture.task.abort();
    Ok(())
}
