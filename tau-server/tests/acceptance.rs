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
            let dir = tempdir()?;
            let socket = dir.path().join("tau.sock");
            let listener = UnixListener::bind(&socket)?;
            let app = tau_server::router(tau_server::AppState::default());
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
while True:
    headers={}
    line=sys.stdin.buffer.readline()
    if not line: break
    while line not in (b'\r\n',b'\n',b''):
        k,v=line.decode().split(':',1); headers[k.lower()]=v.strip()
        line=sys.stdin.buffer.readline()
    if not line: break
    body=sys.stdin.buffer.read(int(headers['content-length']))
    req=json.loads(body); method=req.get('method',''); result={}
    if method == 'tools/list': result={'tools':[{'name':'echo','description':'fixture','input_schema':{}}]}
    elif method == 'prompts/list': result={'prompts':[{'name':'plan','description':'fixture'}]}
    elif method == 'tools/call': result={'content':[{'type':'text','text':'ok'}]}
    elif method.startswith('textDocument/'):
        result=[{'uri':'file:///fixture.rs','range':{'start':{'line':0,'character':0},'end':{'line':0,'character':1}}}]
    response={'jsonrpc':'2.0','id':req.get('id'),'result':result}
    encoded=json.dumps(response).encode(); sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n'%len(encoded)).encode()+encoded); sys.stdout.buffer.flush()
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

use anyhow::Result;
use tau_proto::prelude::*;

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
    let mut client = fixtures::client(&fixture).await?;
    assert_eq!(client.ping().await?, "pong");
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
