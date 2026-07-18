use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use tau_client::TurnStreamEvent;
use tau_proto::prelude::{
    Capability, ClientResponse, IdempotencyKey, ProtocolNegotiateParams, ProtocolVersion,
    RequestAction, TurnCancelParams, TurnDiffDecision, TurnPermissionChoice, TurnReplayParams,
    TurnResponseParams, TurnStartParams,
};
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct Backend {
    socket: PathBuf,
    cwd: String,
    model: String,
    runtime: tokio::runtime::Handle,
    session: Arc<tokio::sync::Mutex<Option<tau_client::Client>>>,
    active: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    active_turn: Arc<Mutex<Option<(tau_client::Client, String, String)>>>,
    _daemon: Arc<Mutex<Option<Child>>>,
    auto_started: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonAction {
    /// Dismiss this warning only; retain ownership for this process.
    Okay,
    /// The caller is closing the GUI; ownership cleanup is performed by Drop.
    Quit,
    /// Release ownership of the child without changing future preferences.
    Disown,
    /// Persist disowned startup behavior and release this child, if owned.
    AlwaysDisown,
    /// Persist hiding this warning, while retaining current ownership policy.
    NeverShowAgain,
}

/// Ownership is explicit: a daemon found before connection is never stopped;
/// only a child returned by `ensure_daemon` is owned by the GUI.
fn stop_child(child: &mut Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(child.id().to_string())
            .status();
    }
    for _ in 0..10 {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
}

impl Drop for Backend {
    fn drop(&mut self) {
        if let Ok(mut daemon) = self._daemon.lock() {
            if let Some(mut child) = daemon.take() {
                stop_child(&mut child);
            }
        }
    }
}

impl Backend {
    pub async fn prepare(socket: &PathBuf) -> Result<(Option<Child>, bool, String, String)> {
        let daemon = ensure_daemon(socket).await?;
        let auto_started = daemon.is_some();
        // An existing daemon is never returned by `ensure_daemon`. If the
        // user chose Always-disown, deliberately leave a newly started child
        // running but do not transfer it into Backend ownership. Keep the
        // auto_started bit separate so warning visibility and ownership stay
        // independent preferences.
        let daemon = if daemon.is_some()
            && crate::preferences::path()
                .ok()
                .and_then(|path| crate::preferences::GuiPreferences::load_from(&path).ok())
                .is_some_and(|preferences| !preferences.daemon_owned)
        {
            None
        } else {
            daemon
        };
        let cwd = std::env::current_dir().context("reading GUI working directory")?;
        let model = tau_core::config::Config::load()
            .ok()
            .and_then(|config| config.model)
            .unwrap_or_else(|| "openai/gpt-4o".into());
        Ok((
            daemon,
            auto_started,
            cwd.to_string_lossy().into_owned(),
            model,
        ))
    }

    pub fn from_parts(
        socket: PathBuf,
        handle: tokio::runtime::Handle,
        daemon: Option<Child>,
        cwd: String,
        model: String,
    ) -> Self {
        let auto_started = daemon.is_some();
        Self::from_parts_with_startup(socket, handle, daemon, auto_started, cwd, model)
    }

    pub fn from_parts_with_startup(
        socket: PathBuf,
        handle: tokio::runtime::Handle,
        daemon: Option<Child>,
        auto_started: bool,
        cwd: String,
        model: String,
    ) -> Self {
        Self {
            socket,
            cwd,
            model,
            runtime: handle,
            session: Arc::new(tokio::sync::Mutex::new(None)),
            active: Arc::new(Mutex::new(None)),
            active_turn: Arc::new(Mutex::new(None)),
            _daemon: Arc::new(Mutex::new(daemon)),
            auto_started,
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    pub fn auto_started(&self) -> bool {
        self.auto_started
            && crate::preferences::path()
                .ok()
                .and_then(|path| crate::preferences::GuiPreferences::load_from(&path).ok())
                .map(|preferences| preferences.daemon_warning)
                .unwrap_or(true)
    }

    /// Apply one of the five ownership decisions. Existing daemons are never
    /// present in `_daemon`, so disowning can only affect a child we spawned.
    pub fn daemon_action(&self, action: DaemonAction) {
        if let Ok(path) = crate::preferences::path() {
            let mut preferences =
                crate::preferences::GuiPreferences::load_from(&path).unwrap_or_default();
            match action {
                DaemonAction::NeverShowAgain => preferences.daemon_warning = false,
                DaemonAction::AlwaysDisown => preferences.daemon_owned = false,
                _ => {}
            }
            let _ = preferences.save_to(&path);
        }
        if matches!(action, DaemonAction::Disown | DaemonAction::AlwaysDisown) {
            if let Ok(mut daemon) = self._daemon.lock() {
                daemon.take();
            }
        }
    }

    /// Start a turn on the long-lived typed client session. The returned
    /// channel contains protocol events, never parsed completion text.
    pub fn turn(
        &self,
        prompt: String,
        session_id: Option<String>,
    ) -> mpsc::UnboundedReceiver<Result<TurnStreamEvent, String>> {
        self.turn_with_agent(prompt, session_id, None)
    }

    pub fn turn_with_agent(
        &self,
        prompt: String,
        session_id: Option<String>,
        agent: Option<String>,
    ) -> mpsc::UnboundedReceiver<Result<TurnStreamEvent, String>> {
        self.turn_with_options(prompt, session_id, agent, None)
    }

    pub fn turn_with_options(
        &self,
        prompt: String,
        session_id: Option<String>,
        agent: Option<String>,
        model: Option<String>,
    ) -> mpsc::UnboundedReceiver<Result<TurnStreamEvent, String>> {
        let (sender, receiver) = mpsc::unbounded_channel();
        let socket = self.socket.clone();
        let cwd = self.cwd.clone();
        let model = model.unwrap_or_else(|| self.model.clone());
        let session = self.session.clone();
        let active_turn = self.active_turn.clone();
        self.cancel();
        let task = self.runtime.spawn(async move {
            let result = async {
                let mut guard = session.lock().await;
                if guard.is_none() {
                    *guard = Some(connect_ready(&socket).await?);
                }
                let client = guard.as_mut().expect("session initialized");
                let params = turn_start_request(model, prompt, session_id, cwd, agent);
                let mut stream = client.turn_start(params).await?;
                while let Some(event) = stream.next().await {
                    let event = event?;
                    if let TurnStreamEvent::Event(ref sequenced) = event {
                        if let tau_proto::prelude::TurnEvent::TurnStarted { turn_id } =
                            &sequenced.event
                        {
                            if let Ok(mut active) = active_turn.lock() {
                                *active = Some((
                                    client.clone(),
                                    sequenced.session_id.clone(),
                                    turn_id.clone(),
                                ));
                            }
                        }
                    }
                    let complete = is_terminal_event(&event);
                    sender
                        .send(Ok(event))
                        .map_err(|_| anyhow::anyhow!("GUI closed"))?;
                    if complete {
                        if let Ok(mut active) = active_turn.lock() {
                            active.take();
                        }
                        break;
                    }
                }
                Ok::<_, anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                let _ = sender.send(Err(error.to_string()));
            }
        });
        if let Ok(mut active) = self.active.lock() {
            *active = Some(task);
        }
        receiver
    }

    /// Stop delivery for the active turn. The daemon owns cancellation; the
    /// next replay/turn remains available on the persistent session.
    pub fn cancel(&self) {
        let active_turn = self.active_turn.clone();
        self.runtime.spawn(async move {
            let active = active_turn.lock().ok().and_then(|mut value| value.take());
            if let Some((client, session_id, turn_id)) = active {
                let _ = client
                    .turn_cancel(cancel_request(session_id, turn_id))
                    .await;
            }
        });
        if let Ok(mut active) = self.active.lock() {
            if let Some(task) = active.take() {
                task.abort();
            }
        }
    }

    pub fn replay(
        &self,
        session_id: String,
        after_sequence: u64,
    ) -> tokio::sync::oneshot::Receiver<Result<tau_proto::prelude::TurnReplayResult, String>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let socket = self.socket.clone();
        self.runtime.spawn(async move {
            let result = async {
                let client = connect_ready(&socket).await?;
                client
                    .turn_replay(replay_request(session_id, after_sequence))
                    .await
            }
            .await
            .map_err(|e: anyhow::Error| e.to_string());
            let _ = tx.send(result);
        });
        rx
    }

    pub fn respond(
        &self,
        session_id: String,
        turn_id: String,
        response: ClientResponse,
    ) -> tokio::sync::oneshot::Receiver<Result<tau_proto::prelude::TurnResponseResult, String>>
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let socket = self.socket.clone();
        self.runtime.spawn(async move {
            let result = async {
                connect_ready(&socket)
                    .await?
                    .turn_response(response_request(session_id, turn_id, response))
                    .await
            }
            .await
            .map_err(|e: anyhow::Error| e.to_string());
            let _ = tx.send(result);
        });
        rx
    }

    pub fn permission_reply(
        &self,
        session_id: String,
        turn_id: String,
        _request_id: String,
        choice: TurnPermissionChoice,
    ) -> tokio::sync::oneshot::Receiver<Result<tau_proto::prelude::TurnResponseResult, String>>
    {
        self.respond(
            session_id,
            turn_id,
            ClientResponse::Permission {
                choice: permission_choice_name(choice).into(),
            },
        )
    }

    pub fn question_reply(
        &self,
        session_id: String,
        turn_id: String,
        answer: String,
    ) -> tokio::sync::oneshot::Receiver<Result<tau_proto::prelude::TurnResponseResult, String>>
    {
        self.respond(session_id, turn_id, ClientResponse::Question { answer })
    }

    pub fn diff_reply(
        &self,
        session_id: String,
        turn_id: String,
        index: u32,
        decision: TurnDiffDecision,
    ) -> tokio::sync::oneshot::Receiver<Result<tau_proto::prelude::TurnResponseResult, String>>
    {
        self.respond(
            session_id,
            turn_id,
            ClientResponse::DiffHunk {
                index,
                approved: matches!(decision, TurnDiffDecision::Approve),
            },
        )
    }
}

fn turn_start_request(
    model: String,
    prompt: String,
    session_id: Option<String>,
    cwd: String,
    agent: Option<String>,
) -> TurnStartParams {
    TurnStartParams {
        model,
        prompt,
        session_id,
        cwd: Some(cwd),
        agent,
        task_tier: None,
        autonomous: None,
        action: Some(RequestAction::Submit),
        idempotency_key: IdempotencyKey::new(format!("gui-{}", unique_nonce())),
    }
}

fn cancel_request(session_id: String, turn_id: String) -> TurnCancelParams {
    TurnCancelParams {
        session_id,
        turn_id,
        idempotency_key: IdempotencyKey::new(format!("gui-cancel-{}", unique_nonce())),
    }
}

fn replay_request(session_id: String, after_sequence: u64) -> TurnReplayParams {
    TurnReplayParams {
        session_id,
        after_sequence,
        limit: Some(500),
    }
}

fn response_request(
    session_id: String,
    turn_id: String,
    response: ClientResponse,
) -> TurnResponseParams {
    TurnResponseParams {
        session_id,
        turn_id,
        idempotency_key: IdempotencyKey::new(format!("gui-response-{}", unique_nonce())),
        response,
    }
}

fn unique_nonce() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

fn permission_choice_name(choice: TurnPermissionChoice) -> &'static str {
    match choice {
        TurnPermissionChoice::AllowOnce => "allow_once",
        TurnPermissionChoice::AllowAlways => "allow_always",
        TurnPermissionChoice::Reject => "reject",
        TurnPermissionChoice::Inspect => "inspect",
        TurnPermissionChoice::Cancel => "cancel",
    }
}

fn is_terminal_event(event: &TurnStreamEvent) -> bool {
    matches!(event, TurnStreamEvent::Complete(_))
}

async fn ensure_daemon(socket: &PathBuf) -> Result<Option<Child>> {
    // A stale/unresponsive socket must not prevent startup indefinitely.
    if daemon_reachable(socket).await {
        return Ok(None);
    }
    let executable = daemon_executable()?;
    let mut child = Command::new(executable)
        .arg("--socket")
        .arg(socket)
        .arg("serve")
        .spawn()
        .context("starting tau daemon")?;
    for _ in 0..50 {
        if daemon_reachable(socket).await {
            return Ok(Some(child));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    stop_child(&mut child);
    anyhow::bail!("daemon did not become ready at {}", socket.display())
}

async fn daemon_reachable(socket: &PathBuf) -> bool {
    tokio::time::timeout(Duration::from_millis(250), connect_ready(socket))
        .await
        .is_ok_and(|result| result.is_ok())
}

async fn connect_ready(socket: &PathBuf) -> Result<tau_client::Client> {
    let client = tau_client::Client::connect(socket).await?;
    let negotiated = tokio::time::timeout(
        Duration::from_millis(250),
        client.negotiate(negotiation_request()),
    )
    .await
    .context("daemon negotiation timed out")??;
    anyhow::ensure!(
        negotiated.version.major == 1,
        "daemon negotiated unsupported major version {}",
        negotiated.version.major
    );
    Ok(client)
}

fn negotiation_request() -> ProtocolNegotiateParams {
    ProtocolNegotiateParams {
        version: ProtocolVersion { major: 1, minor: 0 },
        capabilities: vec![
            Capability::TurnStreaming,
            Capability::TurnCancellation,
            Capability::EventReplay,
            Capability::Idempotency,
            Capability::ArtifactReferences,
        ],
    }
}

fn daemon_executable() -> Result<PathBuf> {
    let current = std::env::current_exe().context("locating GUI executable")?;
    let parent = current
        .parent()
        .context("GUI executable has no parent directory")?;
    let executable = parent.join(format!("tau{}", std::env::consts::EXE_SUFFIX));
    if executable == current {
        anyhow::bail!("refusing to launch GUI executable as daemon")
    }
    Ok(executable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn owned_child_is_stopped_gracefully_before_force_fallback() {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("trap 'exit 0' TERM; sleep 30")
            .spawn()
            .expect("spawn test child");
        stop_child(&mut child);
        assert!(child.try_wait().expect("wait status").is_some());
    }

    #[test]
    fn backend_drop_is_safe_without_owned_daemon() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let backend = Backend::from_parts(
            PathBuf::from("/tmp/does-not-exist"),
            runtime.handle().clone(),
            None,
            "/tmp".into(),
            "test/model".into(),
        );
        drop(backend);
    }

    #[test]
    fn typed_requests_preserve_submit_cancel_and_replay_fields() {
        let start = turn_start_request(
            "m".into(),
            "hello".into(),
            Some("s".into()),
            "/tmp".into(),
            Some("a".into()),
        );
        assert!(matches!(start.action, Some(RequestAction::Submit)));
        assert_eq!(start.session_id.as_deref(), Some("s"));
        let cancel = cancel_request("s".into(), "t".into());
        assert_eq!(
            (cancel.session_id.as_str(), cancel.turn_id.as_str()),
            ("s", "t")
        );
        let replay = replay_request("s".into(), 41);
        assert_eq!(
            (replay.session_id, replay.after_sequence, replay.limit),
            ("s".into(), 41, Some(500))
        );
    }

    #[test]
    fn interactive_responses_remain_typed() {
        let response = response_request(
            "s".into(),
            "t".into(),
            ClientResponse::Question {
                answer: "yes".into(),
            },
        );
        assert_eq!(response.session_id, "s");
        assert_eq!(response.turn_id, "t");
        assert_eq!(
            response.response,
            ClientResponse::Question {
                answer: "yes".into()
            }
        );
    }
}
