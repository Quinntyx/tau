use crate::{
    reducer,
    state::{AppState, Connection},
};
use anyhow::Result;
use futures_util::StreamExt;
use tau_client::{Client, CompletionEvent};
#[derive(Debug, Clone)]
pub enum ClientEvent {
    Delta(String),
    Complete(String),
    Disconnected,
    Reconnected,
}
#[derive(Debug, Default)]
pub struct ScriptedClient {
    pub events: Vec<ClientEvent>,
}
impl ScriptedClient {
    pub fn drive(&self, s: &mut AppState) {
        for e in &self.events {
            match e {
                ClientEvent::Delta(x) => s.transcript.push(x.clone()),
                ClientEvent::Complete(id) => s.session_id = Some(id.clone()),
                ClientEvent::Disconnected => s.connection = Connection::Disconnected,
                ClientEvent::Reconnected => s.connection = Connection::Connected,
            }
        }
    }
}
pub async fn complete(s: &mut AppState, client: &mut Client, prompt: String) -> Result<()> {
    s.transcript.push("tau: ".into());
    let mut stream = client
        .completion_stream(reducer::params(
            s,
            prompt,
            Some(std::env::current_dir()?.to_string_lossy().into_owned()),
        ))
        .await?;
    while let Some(e) = stream.next().await {
        match e? {
            CompletionEvent::Delta(d) => s.transcript.push(d.text),
            CompletionEvent::Complete(r) => s.session_id = Some(r.session_id),
        }
    }
    Ok(())
}
