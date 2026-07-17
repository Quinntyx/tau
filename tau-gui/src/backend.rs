use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use tau_client::TurnStreamEvent;
use tau_proto::prelude::{
    IdempotencyKey, RequestAction, TurnCancelParams, TurnReplayParams, TurnStartParams,
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

/// Typed inputs emitted by the GUI pickers.  These values are identifiers, not
/// presentation labels, and can be recorded or dispatched without inventing
/// a separate wire protocol for picker state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerAction {
    SelectModel(String),
    SelectAgent(String),
    ToggleFavorite(String),
    RecordRecentModel(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnRequest {
    pub prompt: String,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub agent: Option<String>,
}

impl TurnRequest {
    pub fn new(prompt: String, session_id: Option<String>) -> Self {
        Self {
            prompt,
            session_id,
            ..Self::default()
        }
    }

    pub fn apply(&mut self, action: PickerAction) {
        match action {
            PickerAction::SelectModel(model) => self.model = Some(model),
            PickerAction::SelectAgent(agent) => self.agent = Some(agent),
            PickerAction::ToggleFavorite(_) | PickerAction::RecordRecentModel(_) => {}
        }
    }

    fn into_params(self, cwd: String, default_model: String) -> TurnStartParams {
        TurnStartParams {
            model: self.model.unwrap_or(default_model),
            prompt: self.prompt,
            session_id: self.session_id,
            cwd: Some(cwd),
            agent: self.agent,
            task_tier: None,
            autonomous: None,
            action: Some(RequestAction::Submit),
            idempotency_key: IdempotencyKey::new(format!(
                "gui-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos())
            )),
        }
    }
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
        let model = crate::preferences::path()
            .ok()
            .and_then(|path| crate::preferences::GuiPreferences::load_from(&path).ok())
            .and_then(|preferences| preferences.selected_model)
            .unwrap_or(model);
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

    /// Apply a picker event using its stable identifier, and persist GUI-only
    /// state.  Presentation labels are deliberately not interpreted here.
    pub fn apply_picker_action(&mut self, action: PickerAction) -> Result<()> {
        let path = crate::preferences::path()?;
        let mut preferences = crate::preferences::GuiPreferences::load_from(&path)?;
        match action {
            PickerAction::SelectModel(model) => {
                self.model = model.clone();
                preferences.select_model(model);
            }
            PickerAction::SelectAgent(agent) => preferences.select_agent(agent),
            PickerAction::ToggleFavorite(model) => preferences.toggle_favorite(model),
            PickerAction::RecordRecentModel(model) => preferences.record_recent_model(model),
        }
        preferences.save_to(&path)
    }

    pub fn select_model(&mut self, model: impl Into<String>) -> Result<()> {
        self.apply_picker_action(PickerAction::SelectModel(model.into()))
    }

    pub fn select_agent(&mut self, agent: impl Into<String>) -> Result<()> {
        self.apply_picker_action(PickerAction::SelectAgent(agent.into()))
    }

    pub fn toggle_favorite(&mut self, model: impl Into<String>) -> Result<()> {
        self.apply_picker_action(PickerAction::ToggleFavorite(model.into()))
    }

    pub fn record_recent_model(&mut self, model: impl Into<String>) -> Result<()> {
        self.apply_picker_action(PickerAction::RecordRecentModel(model.into()))
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
                    *guard = Some(tau_client::Client::connect(&socket).await?);
                }
                let client = guard.as_mut().expect("session initialized");
                let params = TurnRequest {
                    prompt,
                    session_id,
                    model: Some(model),
                    agent,
                }
                .into_params(cwd, String::new());
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
                    let complete = matches!(event, TurnStreamEvent::Complete(_));
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
                    .turn_cancel(TurnCancelParams {
                        session_id,
                        turn_id,
                        idempotency_key: IdempotencyKey::new(format!(
                            "gui-cancel-{}",
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map_or(0, |d| d.as_nanos())
                        )),
                    })
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
                let client = tau_client::Client::connect(socket).await?;
                client
                    .turn_replay(TurnReplayParams {
                        session_id,
                        after_sequence,
                        limit: Some(500),
                    })
                    .await
            }
            .await
            .map_err(|e: anyhow::Error| e.to_string());
            let _ = tx.send(result);
        });
        rx
    }
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
    tokio::time::timeout(
        Duration::from_millis(250),
        tau_client::Client::connect(socket),
    )
    .await
    .is_ok_and(|result| result.is_ok())
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
    fn picker_actions_build_typed_turn_values() {
        let mut request = TurnRequest::new("hello".into(), Some("session-1".into()));
        request.apply(PickerAction::SelectModel("provider/model".into()));
        request.apply(PickerAction::SelectAgent("build".into()));

        let params = request.into_params("/tmp/project".into(), "fallback/model".into());
        assert_eq!(params.model, "provider/model");
        assert_eq!(params.agent.as_deref(), Some("build"));
        assert_eq!(params.prompt, "hello");
        assert_eq!(params.session_id.as_deref(), Some("session-1"));
        assert_eq!(params.cwd.as_deref(), Some("/tmp/project"));
        assert_eq!(params.action, Some(RequestAction::Submit));
    }

    #[test]
    fn turn_request_uses_default_model_without_label_parsing() {
        let params = TurnRequest::new("hello".into(), None)
            .into_params("/tmp".into(), "default/model".into());
        assert_eq!(params.model, "default/model");
        assert_eq!(params.agent, None);
    }
}
