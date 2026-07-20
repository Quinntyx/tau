use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use tau_client::TurnStreamEvent;
use tau_client::{
    ActiveSessionStore, CreateSession, ProjectId, SessionHistory, SessionRef, SessionRename,
    SessionSummary,
};
use tau_proto::git::{GitAckResult, GitBranchesResult, GitFileResult, GitStatusResult};
use tau_proto::prelude::{
    Capability, ClientResponse, IdempotencyKey, ProtocolNegotiateParams, ProtocolVersion,
    RequestAction, TurnCancelParams, TurnReplayParams, TurnResponseParams, TurnStartParams,
};
use tokio::sync::mpsc;

use crate::picker::PickerAction;

#[derive(Clone)]
pub struct Backend {
    socket: PathBuf,
    cwd: String,
    model: String,
    runtime: tokio::runtime::Handle,
    session: Arc<tokio::sync::Mutex<Option<tau_client::Client>>>,
    active: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    active_turn: Arc<Mutex<Option<(tau_client::Client, String, String)>>>,
    response_context: Arc<Mutex<Option<(tau_client::Client, String, String)>>>,
    _daemon: Arc<Mutex<Option<Child>>>,
    auto_started: bool,
    status: Arc<Mutex<DaemonStatus>>,
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
    /// Retry connection and protocol negotiation without stopping a daemon.
    Retry,
    /// Stop and restart only the daemon owned by this backend.
    Restart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonStatus {
    Absent,
    Spawning,
    Connecting,
    Negotiating,
    Incompatible,
    Degraded,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnRequest {
    pub prompt: String,
    pub session_id: Option<String>,
    pub project_id: String,
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
            PickerAction::ToggleFavorite(_)
            | PickerAction::RecordRecentModel(_)
            | PickerAction::OpenAgents
            | PickerAction::OpenModels
            | PickerAction::Dismiss => {}
        }
    }

    pub fn into_params(self, cwd: String, default_model: String) -> Result<TurnStartParams> {
        if self.project_id.trim().is_empty() {
            anyhow::bail!("cannot start turn: select an active project first")
        }
        Ok(TurnStartParams {
            project_id: self.project_id,
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
        })
    }
}

/// Facts currently known by the backend for sidebar/view-model consumers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BackendSnapshot {
    pub model: Option<String>,
    pub agent: Option<String>,
    pub session: SessionSnapshot,
    pub cwd: Option<String>,
    pub plan: PlanSnapshot,
    pub lsp: ServiceSnapshot,
    pub usage: UsageSnapshot,
    pub mcp: ServiceSnapshot,
    pub task: TaskSnapshot,
    pub startup: StartupSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionSnapshot {
    pub id: Option<String>,
    pub state: SessionState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionState {
    #[default]
    Unavailable,
    Connected,
    Active,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlanSnapshot {
    pub name: Option<String>,
    pub current_step: Option<String>,
    pub airtight: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServiceSnapshot {
    pub available: Option<bool>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UsageSnapshot {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub context_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskSnapshot {
    pub tier: Option<u8>,
    pub autonomous: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StartupSnapshot {
    #[default]
    Loading,
    Ready,
    Unavailable,
    Error(String),
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
    fn require_project_id(project_id: &str) -> Result<()> {
        if project_id.trim().is_empty() {
            anyhow::bail!("cannot access session: select an active project first")
        }
        Ok(())
    }

    async fn typed_client(&self) -> Result<tau_client::Client> {
        let mut guard = self.session.lock().await;
        if guard.is_none() {
            *guard = Some(connect_or_start(&self.socket, &self._daemon).await?);
            guard
                .as_ref()
                .unwrap()
                .negotiate_checked(ProtocolNegotiateParams {
                    version: ProtocolVersion { major: 1, minor: 0 },
                    capabilities: vec![],
                })
                .await?;
        }
        Ok(guard.as_ref().unwrap().clone())
    }

    /// Typed session-navigation operations used by the GPUI view.  Keeping
    /// these behind Backend ensures buttons never mutate the local navigator
    /// without first asking the daemon.
    pub fn session_create(
        &self,
        project_id: ProjectId,
    ) -> tokio::task::JoinHandle<Result<SessionSummary>> {
        let backend = self.clone();
        let cwd = self.cwd.clone();
        self.runtime.spawn(async move {
            Self::require_project_id(project_id.as_str())?;
            let client = backend.typed_client().await?;
            let summary = client
                .session_create(CreateSession {
                    project_id: project_id.clone(),
                    cwd,
                })
                .await?;
            Ok(summary)
        })
    }

    /// Load the daemon-owned project registry.  Callers must use the returned
    /// opaque IDs; deriving IDs from a working directory creates a split-brain
    /// registry and cannot create valid sessions.
    pub async fn project_list(
        &self,
        include_inactive: bool,
    ) -> Result<tau_proto::projects::ProjectListResult> {
        self.typed_client()
            .await?
            .project_list(tau_proto::projects::ProjectListParams {
                include_inactive: Some(include_inactive),
            })
            .await
    }

    pub fn session_list(
        &self,
        project_id: ProjectId,
        include_archived: bool,
    ) -> tokio::task::JoinHandle<Result<Vec<SessionSummary>>> {
        let backend = self.clone();
        self.runtime.spawn(async move {
            Self::require_project_id(project_id.as_str())?;
            let client = backend.typed_client().await?;
            client
                .session_list_with_archived(project_id, include_archived)
                .await
        })
    }

    /// The daemon-backed operations seam.  Keep repository effects here so
    /// the GPUI view only deals in typed results and never touches Git.
    pub fn operations_status(
        &self,
    ) -> tokio::sync::oneshot::Receiver<Result<GitStatusResult, String>> {
        self.operations_call(|client, project| async move {
            client
                .git_status(tau_proto::git::GitStatusParams { project })
                .await
        })
    }

    pub fn session_rename(
        &self,
        params: SessionRename,
    ) -> tokio::task::JoinHandle<Result<SessionSummary>> {
        let backend = self.clone();
        self.runtime.spawn(async move {
            Self::require_project_id(params.project_id.as_str())?;
            backend.typed_client().await?.session_rename(params).await
        })
    }

    pub fn session_archive(
        &self,
        params: SessionRef,
    ) -> tokio::task::JoinHandle<Result<SessionSummary>> {
        let backend = self.clone();
        self.runtime.spawn(async move {
            Self::require_project_id(params.project_id.as_str())?;
            backend.typed_client().await?.session_archive(params).await
        })
    }

    pub fn session_restore(
        &self,
        params: SessionRef,
    ) -> tokio::task::JoinHandle<Result<SessionSummary>> {
        let backend = self.clone();
        self.runtime.spawn(async move {
            Self::require_project_id(params.project_id.as_str())?;
            backend.typed_client().await?.session_restore(params).await
        })
    }

    pub fn session_history(
        &self,
        params: SessionRef,
    ) -> tokio::task::JoinHandle<Result<SessionHistory>> {
        let backend = self.clone();
        self.runtime.spawn(async move {
            Self::require_project_id(params.project_id.as_str())?;
            backend.typed_client().await?.session_history(params).await
        })
    }

    /// The active session is a GUI-local preference; the daemon only owns the
    /// durable session and its event history.
    fn active_session_store() -> Result<ActiveSessionStore> {
        Ok(ActiveSessionStore::new(
            crate::preferences::path()?.with_file_name("active-session.json"),
        ))
    }

    pub fn restore_active_session(
        &self,
        project_id: ProjectId,
    ) -> tokio::task::JoinHandle<Result<Option<SessionSummary>>> {
        let backend = self.clone();
        self.runtime.spawn(async move {
            Self::require_project_id(project_id.as_str())?;
            let store = Self::active_session_store()?;
            let restored = store.restore(&backend.typed_client().await?).await?;
            Ok(restored.filter(|session| session.project_id.as_str() == project_id.as_str()))
        })
    }

    pub fn save_active_session(
        &self,
        project_id: ProjectId,
        session_id: impl Into<String>,
    ) -> tokio::task::JoinHandle<Result<()>> {
        let session_id = session_id.into();
        self.runtime.spawn(async move {
            Self::require_project_id(project_id.as_str())?;
            if session_id.trim().is_empty() {
                anyhow::bail!("cannot save active session: session ID is empty");
            }
            let store = Self::active_session_store()?;
            store.save(&SessionRef {
                project_id,
                session_id: tau_client::SessionId::new(session_id),
            })?;
            Ok(())
        })
    }

    pub fn operations_file(
        &self,
        path: String,
    ) -> tokio::sync::oneshot::Receiver<Result<GitFileResult, String>> {
        self.operations_call(move |client, project| async move {
            client
                .git_file(tau_proto::git::GitFileParams { project, path })
                .await
        })
    }

    pub fn operations_stage(
        &self,
        path: String,
    ) -> tokio::sync::oneshot::Receiver<Result<GitAckResult, String>> {
        self.operations_call(move |client, project| async move {
            client
                .git_stage(tau_proto::git::GitPathParams { project, path })
                .await
        })
    }

    pub fn operations_unstage(
        &self,
        path: String,
    ) -> tokio::sync::oneshot::Receiver<Result<GitAckResult, String>> {
        self.operations_call(move |client, project| async move {
            client
                .git_unstage(tau_proto::git::GitPathParams { project, path })
                .await
        })
    }

    pub fn operations_revert(
        &self,
        path: String,
        confirmed: bool,
    ) -> tokio::sync::oneshot::Receiver<Result<GitAckResult, String>> {
        self.operations_call(move |client, project| async move {
            client
                .git_revert(tau_proto::git::GitRevertParams {
                    project,
                    path,
                    confirmed,
                })
                .await
        })
    }

    pub fn operations_branches(
        &self,
    ) -> tokio::sync::oneshot::Receiver<Result<GitBranchesResult, String>> {
        self.operations_call(|client, project| async move {
            client
                .git_branches(tau_proto::git::GitBranchesParams { project })
                .await
        })
    }

    pub fn operations_create_branch(
        &self,
        name: String,
    ) -> tokio::sync::oneshot::Receiver<Result<GitAckResult, String>> {
        self.operations_call(move |client, project| async move {
            client
                .git_branch_create(tau_proto::git::GitBranchCreateParams { project, name })
                .await
        })
    }

    pub fn operations_switch_branch(
        &self,
        name: String,
    ) -> tokio::sync::oneshot::Receiver<Result<GitAckResult, String>> {
        self.operations_call(move |client, project| async move {
            client
                .git_branch_switch(tau_proto::git::GitBranchSwitchParams { project, name })
                .await
        })
    }

    pub fn operations_ack(
        &self,
        operation: String,
        acknowledged: bool,
    ) -> tokio::sync::oneshot::Receiver<Result<GitAckResult, String>> {
        self.operations_call(move |client, project| async move {
            client
                .git_ack(tau_proto::git::GitAckParams {
                    project,
                    operation,
                    acknowledged,
                })
                .await
        })
    }

    fn operations_call<F, Fut, T>(
        &self,
        call: F,
    ) -> tokio::sync::oneshot::Receiver<Result<T, String>>
    where
        F: FnOnce(tau_client::Client, String) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = anyhow::Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let socket = self.socket.clone();
        let cwd = self.cwd.clone();
        let session = self.session.clone();
        let daemon = self._daemon.clone();
        let status = self.status.clone();
        self.runtime.spawn(async move {
            let result = async {
                let mut guard = session.lock().await;
                if guard.is_none() {
                    status
                        .lock()
                        .map(|mut s| *s = DaemonStatus::Connecting)
                        .ok();
                    *guard = Some(connect_or_start(&socket, &daemon).await?);
                    status
                        .lock()
                        .map(|mut s| *s = DaemonStatus::Negotiating)
                        .ok();
                    let report = guard
                        .as_ref()
                        .unwrap()
                        .negotiate_checked(ProtocolNegotiateParams {
                            version: ProtocolVersion { major: 1, minor: 0 },
                            capabilities: vec![],
                        })
                        .await?;
                    if !report.is_usable() {
                        anyhow::bail!("daemon protocol incompatible")
                    }
                    status.lock().map(|mut s| *s = DaemonStatus::Ready).ok();
                }
                call(guard.as_ref().unwrap().clone(), cwd).await
            }
            .await
            .map_err(|e: anyhow::Error| e.to_string());
            let _ = tx.send(result);
        });
        rx
    }
    /// Return configured agent identifiers from the daemon's shared config
    /// schema. The current protocol has no catalog/list RPC, so this explicit
    /// typed boundary avoids inferring agents from rendered labels while
    /// selections still cross `TurnStartParams.agent` unchanged.
    pub fn configured_agents() -> Vec<crate::picker::AgentOption> {
        tau_core::config::Config::load()
            .map(configured_agent_options)
            .unwrap_or_default()
    }

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
        let initial_status = if daemon.is_some() {
            DaemonStatus::Spawning
        } else {
            DaemonStatus::Absent
        };
        Self {
            socket,
            cwd,
            model,
            runtime: handle,
            session: Arc::new(tokio::sync::Mutex::new(None)),
            active: Arc::new(Mutex::new(None)),
            active_turn: Arc::new(Mutex::new(None)),
            response_context: Arc::new(Mutex::new(None)),
            _daemon: Arc::new(Mutex::new(daemon)),
            auto_started,
            status: Arc::new(Mutex::new(initial_status)),
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
            PickerAction::OpenAgents | PickerAction::OpenModels | PickerAction::Dismiss => {
                anyhow::bail!("non-persistent picker action passed to backend")
            }
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

    pub fn daemon_status(&self) -> DaemonStatus {
        self.status
            .lock()
            .map(|s| *s)
            .unwrap_or(DaemonStatus::Failed)
    }

    /// Return known backend facts without probing or inventing server state.
    pub fn operational_snapshot(&self) -> BackendSnapshot {
        let active = self
            .active_turn
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|(_, session_id, _)| session_id.clone()));
        let session = self.session.try_lock().ok();
        let connected = session.as_ref().is_some_and(|guard| guard.is_some());
        BackendSnapshot {
            model: Some(self.model.clone()),
            session: SessionSnapshot {
                id: active.clone(),
                state: if active.is_some() {
                    SessionState::Active
                } else if connected {
                    SessionState::Connected
                } else {
                    SessionState::Unavailable
                },
            },
            cwd: Some(self.cwd.clone()),
            startup: if connected {
                StartupSnapshot::Ready
            } else {
                StartupSnapshot::Unavailable
            },
            ..BackendSnapshot::default()
        }
    }

    pub fn snapshot(&self) -> BackendSnapshot {
        self.operational_snapshot()
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
        if matches!(action, DaemonAction::Retry) {
            let session = self.session.clone();
            self.runtime.spawn(async move {
                *session.lock().await = None;
            });
            self.status
                .lock()
                .map(|mut s| *s = DaemonStatus::Connecting)
                .ok();
        }
        if matches!(action, DaemonAction::Restart) {
            if let Ok(mut daemon) = self._daemon.lock() {
                if let Some(mut child) = daemon.take() {
                    stop_child(&mut child);
                }
            }
            let session = self.session.clone();
            self.runtime.spawn(async move {
                *session.lock().await = None;
            });
            self.status
                .lock()
                .map(|mut s| *s = DaemonStatus::Absent)
                .ok();
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

    pub fn turn_with_project(
        &self,
        prompt: String,
        session_id: Option<String>,
        project_id: String,
    ) -> mpsc::UnboundedReceiver<Result<TurnStreamEvent, String>> {
        self.turn_with_project_options(prompt, session_id, None, None, Some(project_id))
    }

    pub fn turn_with_options(
        &self,
        prompt: String,
        session_id: Option<String>,
        agent: Option<String>,
        model: Option<String>,
    ) -> mpsc::UnboundedReceiver<Result<TurnStreamEvent, String>> {
        self.turn_with_project_options(prompt, session_id, agent, model, None)
    }

    /// Start a turn for an explicitly selected active project.
    pub fn turn_with_project_options(
        &self,
        prompt: String,
        session_id: Option<String>,
        agent: Option<String>,
        model: Option<String>,
        project_id: Option<String>,
    ) -> mpsc::UnboundedReceiver<Result<TurnStreamEvent, String>> {
        let (sender, receiver) = mpsc::unbounded_channel();
        if project_id
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            let _ = sender.send(Err(
                "cannot start turn: select an active project first".to_string()
            ));
            return receiver;
        }
        let socket = self.socket.clone();
        let cwd = self.cwd.clone();
        let model = model.unwrap_or_else(|| self.model.clone());
        let session = self.session.clone();
        let active_turn = self.active_turn.clone();
        let response_context = self.response_context.clone();
        let status = self.status.clone();
        let failed_session = session.clone();
        let daemon = self._daemon.clone();
        self.cancel();
        let task = self.runtime.spawn(async move {
            let result = async {
                let mut guard = session.lock().await;
                if guard.is_none() {
                    status
                        .lock()
                        .map(|mut s| *s = DaemonStatus::Connecting)
                        .ok();
                    *guard = Some(connect_or_start(&socket, &daemon).await?);
                    status
                        .lock()
                        .map(|mut s| *s = DaemonStatus::Negotiating)
                        .ok();
                    let report = match tokio::time::timeout(
                        Duration::from_secs(1),
                        guard
                            .as_ref()
                            .expect("session initialized")
                            .negotiate_checked(ProtocolNegotiateParams {
                                version: ProtocolVersion { major: 1, minor: 0 },
                                capabilities: vec![
                                    Capability::TurnStreaming,
                                    Capability::TurnCancellation,
                                    Capability::Idempotency,
                                    Capability::EventReplay,
                                ],
                            }),
                    )
                    .await
                    .context("protocol negotiation timed out")?
                    {
                        Ok(report) => report,
                        Err(error) => {
                            status
                                .lock()
                                .map(|mut s| *s = DaemonStatus::Incompatible)
                                .ok();
                            guard.take();
                            return Err(error);
                        }
                    };
                    if report.compatibility == tau_client::ProtocolCompatibility::Incompatible {
                        status
                            .lock()
                            .map(|mut s| *s = DaemonStatus::Incompatible)
                            .ok();
                        guard.take();
                        anyhow::bail!(
                            "daemon protocol incompatible; missing {:?}",
                            report.missing_capabilities
                        );
                    }
                    status
                        .lock()
                        .map(|mut s| {
                            *s = if report.compatibility
                                == tau_client::ProtocolCompatibility::Degraded
                            {
                                DaemonStatus::Degraded
                            } else {
                                DaemonStatus::Ready
                            }
                        })
                        .ok();
                }
                let client = guard.as_mut().expect("session initialized");
                let params = TurnRequest {
                    prompt,
                    session_id,
                    project_id: project_id.expect("validated project selection"),
                    model: Some(model),
                    agent,
                }
                .into_params(cwd, String::new())?;
                let mut stream = client.turn_start(params).await?;
                while let Some(event) = stream.next().await {
                    let event = event?;
                    if let TurnStreamEvent::Event(ref sequenced) = event {
                        if let tau_proto::prelude::TurnEvent::TurnStarted { turn_id } =
                            &sequenced.event
                        {
                            if let Ok(mut active) = active_turn.lock() {
                                let context = (
                                    client.clone(),
                                    sequenced.session_id.clone(),
                                    turn_id.clone(),
                                );
                                *active = Some(context.clone());
                                if let Ok(mut response) = response_context.lock() {
                                    *response = Some(context);
                                }
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
                *failed_session.lock().await = None;
                if status
                    .lock()
                    .map(|s| *s != DaemonStatus::Incompatible)
                    .unwrap_or(true)
                {
                    status.lock().map(|mut s| *s = DaemonStatus::Failed).ok();
                }
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

    /// Submit an interactive decision on the typed response channel. The
    /// transcript card is only a projection; acknowledgement belongs to the
    /// daemon and is not faked by the view.
    pub fn respond(
        &self,
        response: ClientResponse,
    ) -> tokio::sync::oneshot::Receiver<Result<(), String>> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let response_context = self.response_context.clone();
        self.runtime.spawn(async move {
            let (kind, request_id) = match &response {
                ClientResponse::Permission { request_id, .. } => {
                    ("permission", request_id.as_str())
                }
                ClientResponse::Question { question_id, .. } => ("question", question_id.as_str()),
                ClientResponse::DiffHunk { request_id, .. } => ("diff", request_id.as_str()),
            };
            let active = response_context.lock().ok().and_then(|value| value.clone());
            let result = if let Some((client, session_id, turn_id)) = active {
                client
                    .turn_response(TurnResponseParams {
                        session_id,
                        turn_id,
                        idempotency_key: IdempotencyKey::new(format!(
                            "gui-response-{kind}-{request_id}"
                        )),
                        response,
                    })
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|ack| {
                        ack.accepted
                            .then_some(())
                            .ok_or_else(|| "daemon rejected interactive response".to_string())
                    })
            } else {
                Err("no active turn for interactive response".to_string())
            };
            let _ = sender.send(result);
        });
        receiver
    }

    pub fn replay(
        &self,
        session_id: String,
        after_sequence: u64,
    ) -> tokio::sync::oneshot::Receiver<Result<tau_proto::prelude::TurnReplayResult, String>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let socket = self.socket.clone();
        let session = self.session.clone();
        let status = self.status.clone();
        let daemon = self._daemon.clone();
        self.runtime.spawn(async move {
            let result = async {
                let mut guard = session.lock().await;
                if guard.is_none() {
                    status
                        .lock()
                        .map(|mut s| *s = DaemonStatus::Connecting)
                        .ok();
                    *guard = Some(connect_or_start(&socket, &daemon).await?);
                    status
                        .lock()
                        .map(|mut s| *s = DaemonStatus::Negotiating)
                        .ok();
                    let report = match guard
                        .as_ref()
                        .unwrap()
                        .negotiate_checked(ProtocolNegotiateParams {
                            version: ProtocolVersion { major: 1, minor: 0 },
                            capabilities: vec![Capability::EventReplay],
                        })
                        .await
                    {
                        Ok(report) => report,
                        Err(error) => {
                            status
                                .lock()
                                .map(|mut s| *s = DaemonStatus::Incompatible)
                                .ok();
                            guard.take();
                            return Err(error);
                        }
                    };
                    if !report.is_usable() {
                        status
                            .lock()
                            .map(|mut s| *s = DaemonStatus::Incompatible)
                            .ok();
                        guard.take();
                        anyhow::bail!("daemon protocol incompatible")
                    }
                    status
                        .lock()
                        .map(|mut s| {
                            *s = if report.compatibility
                                == tau_client::ProtocolCompatibility::Degraded
                            {
                                DaemonStatus::Degraded
                            } else {
                                DaemonStatus::Ready
                            }
                        })
                        .ok();
                }
                let client = guard.as_ref().unwrap();
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

fn configured_agent_options(config: tau_core::config::Config) -> Vec<crate::picker::AgentOption> {
    config
        .agents
        .into_iter()
        .map(|(name, agent)| crate::picker::AgentOption {
            name,
            in_tab_cycle: agent.cycle,
        })
        .collect()
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

async fn connect_or_start(
    socket: &PathBuf,
    owned_daemon: &Arc<Mutex<Option<Child>>>,
) -> Result<tau_client::Client> {
    match tau_client::Client::connect(socket).await {
        Ok(client) => Ok(client),
        Err(connect_error) => match ensure_daemon(socket).await? {
            Some(child) => {
                if let Ok(mut daemon) = owned_daemon.lock() {
                    *daemon = Some(child);
                }
                tau_client::Client::connect(socket)
                    .await
                    .context("connecting to newly started tau daemon")
            }
            None => Err(connect_error),
        },
    }
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

    #[test]
    fn snapshot_defaults_are_explicitly_unavailable() {
        let snapshot = BackendSnapshot::default();
        assert_eq!(snapshot.session.state, SessionState::Unavailable);
        assert_eq!(snapshot.lsp.available, None);
        assert_eq!(snapshot.usage.total_tokens, None);
        assert!(matches!(snapshot.startup, StartupSnapshot::Loading));
    }

    #[test]
    fn snapshot_preserves_unavailable_server_facts() {
        let snapshot = BackendSnapshot {
            model: None,
            cwd: None,
            startup: StartupSnapshot::Unavailable,
            ..Default::default()
        };
        assert_eq!(snapshot.model, None);
        assert_eq!(snapshot.session.id, None);
        assert_eq!(snapshot.plan.airtight, None);
    }

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
    fn fresh_backend_reports_absent_until_connection_is_negotiated() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let backend = Backend::from_parts(
            PathBuf::from("/tmp/does-not-exist"),
            runtime.handle().clone(),
            None,
            "/tmp".into(),
            "test/model".into(),
        );
        assert_eq!(backend.daemon_status(), DaemonStatus::Absent);
    }

    #[test]
    fn picker_actions_build_typed_turn_values() {
        let mut request = TurnRequest::new("hello".into(), Some("session-1".into()));
        request.project_id = "project-1".into();
        request.apply(PickerAction::SelectModel("provider/model".into()));
        request.apply(PickerAction::SelectAgent("build".into()));

        let params = request
            .into_params("/tmp/project".into(), "fallback/model".into())
            .unwrap();
        assert_eq!(params.model, "provider/model");
        assert_eq!(params.agent.as_deref(), Some("build"));
        assert_eq!(params.prompt, "hello");
        assert_eq!(params.session_id.as_deref(), Some("session-1"));
        assert_eq!(params.cwd.as_deref(), Some("/tmp/project"));
        assert_eq!(params.action, Some(RequestAction::Submit));
    }

    #[test]
    fn turn_request_uses_default_model_without_label_parsing() {
        let mut request = TurnRequest::new("hello".into(), None);
        request.project_id = "project-1".into();
        let params = request
            .into_params("/tmp".into(), "default/model".into())
            .unwrap();
        assert_eq!(params.model, "default/model");
        assert_eq!(params.agent, None);
        assert_eq!(params.project_id, "project-1");
    }

    #[test]
    fn turn_request_rejects_missing_project_selection() {
        let error = TurnRequest::new("hello".into(), None)
            .into_params("/tmp".into(), "default/model".into())
            .unwrap_err();
        assert!(error.to_string().contains("select an active project"));
    }

    #[test]
    fn configured_agent_boundary_preserves_server_configured_names_and_cycle_flags() {
        let mut config = tau_core::config::Config::default();
        config.agents.insert(
            "reviewer".into(),
            tau_core::config::AgentConfig {
                cycle: true,
                ..Default::default()
            },
        );
        config.agents.insert(
            "archive".into(),
            tau_core::config::AgentConfig {
                cycle: false,
                ..Default::default()
            },
        );
        let agents = configured_agent_options(config);
        assert_eq!(agents[0].name, "archive");
        assert!(!agents[0].in_tab_cycle);
        assert_eq!(agents[1].name, "reviewer");
        assert!(agents[1].in_tab_cycle);
    }
}
