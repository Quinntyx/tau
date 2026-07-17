//! Local browser GUI for tau model turns.

use std::path::PathBuf;
use std::process::{Child, Command};

use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde::Deserialize;
use tau_client::CompletionEvent;
use tau_proto::prelude::{CompletionStreamParams, CompletionStreamResult};
use tokio::net::TcpListener;

#[derive(Clone)]
struct AppState {
    socket: PathBuf,
    cwd: String,
    model: String,
}

#[derive(Debug, Deserialize)]
struct CompletionInput {
    prompt: String,
    model: Option<String>,
    session_id: Option<String>,
}

pub async fn run(socket: PathBuf) -> Result<()> {
    let _daemon = ensure_daemon(&socket).await?;
    let cwd = std::env::current_dir().context("reading GUI working directory")?;
    let model = tau_core::config::Config::load()
        .ok()
        .and_then(|config| config.model)
        .unwrap_or_else(|| "openai/gpt-4o".into());
    let state = AppState {
        socket,
        cwd: cwd.to_string_lossy().into_owned(),
        model,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let url = format!("http://{address}/");
    println!("tau GUI: {url}");
    open_browser(&url);
    let app = Router::new()
        .route("/", get(index))
        .route("/api/completion", post(completion))
        .with_state(state);
    axum::serve(listener, app).await.context("GUI server")?;
    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(INDEX)
}

async fn completion(
    State(state): State<AppState>,
    Json(input): Json<CompletionInput>,
) -> Result<Json<CompletionStreamResult>, GuiError> {
    let mut client = tau_client::Client::connect(&state.socket)
        .await
        .map_err(GuiError::from)?;
    let params = CompletionStreamParams {
        model: input.model.unwrap_or(state.model),
        prompt: input.prompt,
        session_id: input.session_id,
        cwd: Some(state.cwd),
    };
    let mut stream = client
        .completion_stream(params)
        .await
        .map_err(GuiError::from)?;
    while let Some(event) = stream.next().await {
        if let CompletionEvent::Complete(result) = event.map_err(GuiError::from)? {
            return Ok(Json(result));
        }
    }
    Err(GuiError::message(
        "completion ended without a final response",
    ))
}

#[derive(Debug)]
struct GuiError(anyhow::Error);

impl GuiError {
    fn message(message: impl Into<String>) -> Self {
        Self(anyhow::anyhow!(message.into()))
    }
}

impl From<anyhow::Error> for GuiError {
    fn from(error: anyhow::Error) -> Self {
        Self(error)
    }
}

impl IntoResponse for GuiError {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, self.0.to_string()).into_response()
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
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    anyhow::bail!("daemon did not become ready at {}", socket.display())
}

fn open_browser(url: &str) {
    #[cfg(target_os = "linux")]
    let _ = Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = Command::new("cmd").args(["/C", "start", url]).spawn();
}

const INDEX: &str = r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>tau</title><style>
:root{color-scheme:dark;font:16px/1.5 system-ui,sans-serif;background:#111318;color:#eceff4}
body{max-width:900px;margin:0 auto;padding:32px 20px}h1{font-size:28px;margin:0 0 24px}
#transcript{min-height:55vh;display:flex;flex-direction:column;gap:14px;margin-bottom:20px}
.msg{padding:14px 16px;border-radius:12px;white-space:pre-wrap}.user{align-self:flex-end;background:#2d5b91;max-width:80%}.assistant{background:#20242c;border:1px solid #343a46;max-width:90%}
form{display:flex;gap:10px;position:sticky;bottom:20px}textarea{flex:1;min-height:60px;resize:vertical;background:#1a1d24;border:1px solid #454c5a;border-radius:10px;color:inherit;padding:12px;font:inherit}button{border:0;border-radius:10px;background:#86b7ff;color:#10131a;padding:0 20px;font-weight:700;cursor:pointer}button:disabled{opacity:.5}
.meta{color:#9da7b8;font-size:12px;margin:8px 0 20px}
</style></head><body><h1>tau</h1><div class="meta">Local model-turn console</div><main id="transcript"></main><form><textarea placeholder="Ask tau anything..." autofocus></textarea><button>Send</button></form>
<script>const t=document.querySelector('#transcript'),f=document.querySelector('form'),q=document.querySelector('textarea'),b=document.querySelector('button');let session=null;
function add(text,kind){const e=document.createElement('div');e.className='msg '+kind;e.textContent=text;t.append(e);e.scrollIntoView();return e}
f.onsubmit=async e=>{e.preventDefault();const prompt=q.value.trim();if(!prompt)return;add(prompt,'user');q.value='';b.disabled=true;const out=add('Thinking…','assistant');try{const r=await fetch('/api/completion',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({prompt,session_id:session})});if(!r.ok)throw new Error(await r.text());const result=await r.json();session=result.session_id;out.textContent=result.text}catch(error){out.textContent='Error: '+error.message}finally{b.disabled=false;q.focus()}}
</script></body></html>"##;
