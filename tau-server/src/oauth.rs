use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tau_core::codex;
use tau_core::credentials::CredentialStore;
use tau_proto::prelude::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, oneshot};

#[derive(Clone)]
pub(crate) struct OAuthManager {
    inner: Arc<Mutex<OAuthState>>,
}

struct OAuthState {
    status: AuthState,
    cancel: Option<oneshot::Sender<()>>,
}

impl OAuthManager {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(OAuthState {
                status: AuthState::SignedOut,
                cancel: None,
            })),
        }
    }

    pub(crate) async fn status(&self) -> AuthStatusResult {
        let mut inner = self.inner.lock().await;
        if !matches!(inner.status, AuthState::Pending { .. }) {
            inner.status = persisted_status();
        }
        auth_result(inner.status.clone())
    }

    pub(crate) async fn begin(&self) -> Result<AuthBeginResult> {
        let store = CredentialStore::new()?;
        let _ = store
            .get_secure(OPENAI_CODEX_PROVIDER)
            .context("Codex OAuth requires an available operating-system keyring")?;

        self.cancel().await;
        let listener = TcpListener::bind(("127.0.0.1", codex::CALLBACK_PORT))
            .await
            .context("opening Codex OAuth callback on localhost:1455")?;
        let pkce = codex::generate_pkce();
        let state = codex::generate_state();
        let authorize_url = codex::authorize_url(&pkce, &state)?;
        let (cancel_tx, cancel_rx) = oneshot::channel();
        {
            let mut inner = self.inner.lock().await;
            inner.status = AuthState::Pending {
                authorize_url: authorize_url.clone(),
            };
            inner.cancel = Some(cancel_tx);
        }

        let manager = self.clone();
        tokio::spawn(async move {
            let completed = tokio::select! {
                _ = cancel_rx => None,
                result = tokio::time::timeout(Duration::from_secs(300), listener.accept()) => Some(result),
            };
            let Some(completed) = completed else { return };
            let status = match completed {
                Ok(Ok((stream, _))) => complete_callback(stream, &pkce, &state).await,
                Ok(Err(error)) => Err(error).context("accepting Codex OAuth callback"),
                Err(_) => Err(anyhow::anyhow!(
                    "Codex authorization timed out after five minutes"
                )),
            };
            let mut inner = manager.inner.lock().await;
            inner.cancel = None;
            inner.status = match status {
                Ok(tokens) => AuthState::SignedIn {
                    email: tokens.email,
                    account_id: tokens.account_id,
                },
                Err(error) => AuthState::Failed {
                    message: error.to_string(),
                },
            };
        });
        Ok(auth_result(AuthState::Pending { authorize_url }))
    }

    pub(crate) async fn cancel(&self) -> AuthCancelResult {
        let mut inner = self.inner.lock().await;
        if let Some(cancel) = inner.cancel.take() {
            let _ = cancel.send(());
        }
        inner.status = persisted_status();
        auth_result(inner.status.clone())
    }

    pub(crate) async fn logout(&self) -> Result<AuthLogoutResult> {
        self.cancel().await;
        codex::logout(&CredentialStore::new()?)?;
        let mut inner = self.inner.lock().await;
        inner.status = AuthState::SignedOut;
        Ok(auth_result(inner.status.clone()))
    }
}

fn persisted_status() -> AuthState {
    match CredentialStore::new().and_then(|store| codex::load_tokens(&store)) {
        Ok(Some(tokens)) => AuthState::SignedIn {
            email: tokens.email,
            account_id: tokens.account_id,
        },
        Ok(None) => AuthState::SignedOut,
        Err(error) => AuthState::Failed {
            message: format!("Secure account storage is unavailable: {error}"),
        },
    }
}

async fn complete_callback(
    mut stream: TcpStream,
    pkce: &codex::PkceCodes,
    expected_state: &str,
) -> Result<codex::CodexTokenBundle> {
    let mut bytes = vec![0; 8 * 1024];
    let read = stream
        .read(&mut bytes)
        .await
        .context("reading Codex OAuth callback")?;
    let request = String::from_utf8_lossy(&bytes[..read]);
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .context("invalid OAuth callback request")?;
    let url = url::Url::parse(&format!(
        "http://localhost:{}{target}",
        codex::CALLBACK_PORT
    ))?;
    let params = url
        .query_pairs()
        .collect::<std::collections::BTreeMap<_, _>>();
    let result: Result<codex::CodexTokenBundle> =
        if params.get("state").map(|value| value.as_ref()) != Some(expected_state) {
            Err(anyhow::anyhow!(
                "OAuth state did not match; authorization was rejected"
            ))
        } else if let Some(error) = params
            .get("error_description")
            .or_else(|| params.get("error"))
        {
            Err(anyhow::anyhow!(
                "ChatGPT authorization was rejected: {error}"
            ))
        } else {
            match params.get("code") {
                Some(code) => codex::exchange_code(code, pkce).await,
                None => Err(anyhow::anyhow!(
                    "OAuth callback omitted the authorization code"
                )),
            }
        };
    let (status, title, message) = if result.is_ok() {
        (
            "200 OK",
            "Tau is connected",
            "You can close this window and return to Tau.",
        )
    } else {
        (
            "400 Bad Request",
            "Tau could not connect",
            "Return to Tau for details and try again.",
        )
    };
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>{title}</title><style>body{{font:16px system-ui;max-width:36rem;margin:15vh auto;padding:2rem;color:#1d1d1f}}h1{{font-size:28px}}</style><h1>{title}</h1><p>{message}</p>"
    );
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let tokens = result?;
    codex::save_tokens(&CredentialStore::new()?, &tokens)?;
    Ok(tokens)
}

fn auth_result(status: AuthState) -> AuthStatusResult {
    AuthStatusResult {
        provider: OPENAI_CODEX_PROVIDER.into(),
        status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn callback_rejects_mismatched_state_before_token_exchange() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let client = tokio::spawn(async move {
            let mut stream = TcpStream::connect(address).await.unwrap();
            stream
                .write_all(
                    b"GET /auth/callback?code=secret&state=wrong HTTP/1.1\r\nHost: localhost\r\n\r\n",
                )
                .await
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).await.unwrap();
            response
        });
        let (stream, _) = listener.accept().await.unwrap();
        let error = complete_callback(
            stream,
            &codex::PkceCodes {
                verifier: "verifier".into(),
                challenge: "challenge".into(),
            },
            "expected",
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("state did not match"));
        assert!(
            client
                .await
                .unwrap()
                .starts_with("HTTP/1.1 400 Bad Request")
        );
    }
}
