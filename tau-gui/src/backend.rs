use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use tau_client::CompletionEvent;
use tau_proto::prelude::CompletionStreamParams;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct Backend {
    socket: PathBuf,
    cwd: String,
    model: String,
    runtime: tokio::runtime::Handle,
    _daemon: Arc<Mutex<Option<Child>>>,
    auto_started: bool,
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

    pub fn completion(
        &self,
        prompt: String,
        session_id: Option<String>,
    ) -> mpsc::UnboundedReceiver<Result<CompletionEvent, String>> {
        let (sender, receiver) = mpsc::unbounded_channel();
        let socket = self.socket.clone();
        let cwd = self.cwd.clone();
        let model = self.model.clone();
        self.runtime.spawn(async move {
            let result = async {
                let mut client = tau_client::Client::connect(&socket).await?;
                let params = CompletionStreamParams {
                    model,
                    prompt,
                    session_id,
                    cwd: Some(cwd),
                };
                let mut stream = client.completion_stream(params).await?;
                while let Some(event) = stream.next().await {
                    let event = event?;
                    let complete = matches!(event, CompletionEvent::Complete(_));
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
        receiver
    }
}

async fn ensure_daemon(socket: &PathBuf) -> Result<Option<Child>> {
    if tau_client::Client::connect(socket).await.is_ok() {
        return Ok(None);
    }
    let executable = std::env::current_exe().context("locating tau executable")?;
    let child = Command::new(executable)
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
    anyhow::bail!("daemon did not become ready at {}", socket.display())
}
