use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use tau_client::TurnStreamEvent;
use tau_proto::prelude::{IdempotencyKey, TurnStartParams};
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct Backend {
    socket: PathBuf,
    cwd: String,
    model: String,
    runtime: tokio::runtime::Handle,
    session: Arc<tokio::sync::Mutex<Option<tau_client::Client>>>,
    active: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    _daemon: Arc<Mutex<Option<Child>>>,
    auto_started: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonAction {
    Okay,
    Quit,
    Disown,
    AlwaysDisown,
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
    pub async fn prepare(socket: &PathBuf) -> Result<(Option<Child>, String, String)> {
        let daemon = ensure_daemon(socket).await?;
        let cwd = std::env::current_dir().context("reading GUI working directory")?;
        let model = tau_core::config::Config::load()
            .ok()
            .and_then(|config| config.model)
            .unwrap_or_else(|| "openai/gpt-4o".into());
        Ok((daemon, cwd.to_string_lossy().into_owned(), model))
    }

    pub fn from_parts(
        socket: PathBuf,
        handle: tokio::runtime::Handle,
        daemon: Option<Child>,
        cwd: String,
        model: String,
    ) -> Self {
        let auto_started = daemon.is_some();
        Self {
            socket,
            cwd,
            model,
            runtime: handle,
            session: Arc::new(tokio::sync::Mutex::new(None)),
            active: Arc::new(Mutex::new(None)),
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
    }

    /// Apply one of the five ownership decisions. Existing daemons are never
    /// present in `_daemon`, so disowning can only affect a child we spawned.
    pub fn daemon_action(&self, action: DaemonAction) {
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
        let (sender, receiver) = mpsc::unbounded_channel();
        let socket = self.socket.clone();
        let cwd = self.cwd.clone();
        let model = self.model.clone();
        let session = self.session.clone();
        self.cancel();
        let task = self.runtime.spawn(async move {
            let result = async {
                let mut guard = session.lock().await;
                if guard.is_none() {
                    *guard = Some(tau_client::Client::connect(&socket).await?);
                }
                let client = guard.as_mut().expect("session initialized");
                let params = TurnStartParams {
                    model,
                    prompt,
                    session_id,
                    cwd: Some(cwd),
                    idempotency_key: IdempotencyKey::new(format!(
                        "gui-{}",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |d| d.as_nanos())
                    )),
                };
                let mut stream = client.turn_start(params).await?;
                while let Some(event) = stream.next().await {
                    let event = event?;
                    let complete = matches!(event, TurnStreamEvent::Complete(_));
                    sender
                        .send(Ok(event))
                        .map_err(|_| anyhow::anyhow!("GUI closed"))?;
                    if complete {
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
        if let Ok(mut active) = self.active.lock() {
            if let Some(task) = active.take() {
                task.abort();
            }
        }
    }
}

async fn ensure_daemon(socket: &PathBuf) -> Result<Option<Child>> {
    if tau_client::Client::connect(socket).await.is_ok() {
        return Ok(None);
    }
    let executable = std::env::current_exe().context("locating tau executable")?;
    let mut child = Command::new(executable)
        .arg("--socket")
        .arg(socket)
        .arg("serve")
        .spawn()
        .context("starting tau daemon")?;
    for _ in 0..50 {
        if tau_client::Client::connect(socket).await.is_ok() {
            return Ok(Some(child));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    stop_child(&mut child);
    anyhow::bail!("daemon did not become ready at {}", socket.display())
}
