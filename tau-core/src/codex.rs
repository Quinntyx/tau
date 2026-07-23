//! ChatGPT Codex OAuth primitives shared by the daemon and provider adapter.
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tau_proto::auth::OPENAI_CODEX_PROVIDER;
use url::Url;
use uuid::Uuid;

use crate::credentials::CredentialStore;

pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const ISSUER: &str = "https://auth.openai.com";
pub const CODEX_API_BASE: &str = "https://chatgpt.com/backend-api/codex";
pub const CALLBACK_PORT: u16 = 1455;
pub const CALLBACK_URL: &str = "http://localhost:1455/auth/callback";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexTokenBundle {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkceCodes {
    pub verifier: String,
    pub challenge: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    id_token: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct JwtClaims {
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    chatgpt_account_id: Option<String>,
    #[serde(default, rename = "https://api.openai.com/auth")]
    openai_auth: Option<OpenAiClaims>,
    #[serde(default)]
    organizations: Vec<OrganizationClaim>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenAiClaims {
    #[serde(default)]
    chatgpt_account_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OrganizationClaim {
    id: String,
}

pub fn generate_pkce() -> PkceCodes {
    let verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    PkceCodes {
        verifier,
        challenge,
    }
}

pub fn generate_state() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

pub fn authorize_url(pkce: &PkceCodes, state: &str) -> Result<String> {
    let mut url = Url::parse(&format!("{ISSUER}/oauth/authorize"))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", CALLBACK_URL)
        .append_pair("scope", "openid profile email offline_access")
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("state", state)
        .append_pair("originator", "tau");
    Ok(url.into())
}

pub async fn exchange_code(code: &str, pkce: &PkceCodes) -> Result<CodexTokenBundle> {
    exchange_code_at(&format!("{ISSUER}/oauth/token"), code, pkce).await
}

async fn exchange_code_at(
    token_url: &str,
    code: &str,
    pkce: &PkceCodes,
) -> Result<CodexTokenBundle> {
    let response = reqwest::Client::new()
        .post(token_url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", CALLBACK_URL),
            ("client_id", CLIENT_ID),
            ("code_verifier", pkce.verifier.as_str()),
        ])
        .send()
        .await
        .context("exchanging Codex authorization code")?;
    decode_token_response(response, None).await
}

pub async fn fresh_tokens(store: &CredentialStore) -> Result<CodexTokenBundle> {
    let serialized = store
        .get_secure(OPENAI_CODEX_PROVIDER)?
        .context("ChatGPT account is not connected")?;
    let current: CodexTokenBundle =
        serde_json::from_str(&serialized).context("decoding Codex credential")?;
    if current.expires_at_ms > now_ms().saturating_add(60_000) {
        return Ok(current);
    }
    let response = reqwest::Client::new()
        .post(format!("{ISSUER}/oauth/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", current.refresh_token.as_str()),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .await
        .context("refreshing Codex access token")?;
    let refreshed = decode_token_response(response, Some(&current)).await?;
    save_tokens(store, &refreshed)?;
    Ok(refreshed)
}

pub fn save_tokens(store: &CredentialStore, tokens: &CodexTokenBundle) -> Result<()> {
    store.set_secure(
        OPENAI_CODEX_PROVIDER,
        &serde_json::to_string(tokens).context("encoding Codex credential")?,
    )
}

pub fn load_tokens(store: &CredentialStore) -> Result<Option<CodexTokenBundle>> {
    store
        .get_secure(OPENAI_CODEX_PROVIDER)?
        .map(|serialized| serde_json::from_str(&serialized).context("decoding Codex credential"))
        .transpose()
}

pub fn logout(store: &CredentialStore) -> Result<()> {
    store.delete_secure(OPENAI_CODEX_PROVIDER)
}

async fn decode_token_response(
    response: reqwest::Response,
    previous: Option<&CodexTokenBundle>,
) -> Result<CodexTokenBundle> {
    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await.unwrap_or_default();
        anyhow::bail!("Codex token request failed ({status}): {detail}")
    }
    let response: TokenResponse = response.json().await.context("decoding Codex tokens")?;
    let claims = response
        .id_token
        .as_deref()
        .and_then(parse_claims)
        .or_else(|| parse_claims(&response.access_token))
        .unwrap_or_default();
    let account_id = claims
        .chatgpt_account_id
        .or_else(|| {
            claims
                .openai_auth
                .and_then(|claims| claims.chatgpt_account_id)
        })
        .or_else(|| {
            claims
                .organizations
                .first()
                .map(|organization| organization.id.clone())
        })
        .or_else(|| previous.and_then(|tokens| tokens.account_id.clone()));
    Ok(CodexTokenBundle {
        access_token: response.access_token,
        refresh_token: response
            .refresh_token
            .or_else(|| previous.map(|tokens| tokens.refresh_token.clone()))
            .context("Codex token response omitted refresh token")?,
        expires_at_ms: now_ms().saturating_add(response.expires_in.unwrap_or(3_600) * 1_000),
        account_id,
        email: claims
            .email
            .or_else(|| previous.and_then(|tokens| tokens.email.clone())),
    })
}

fn parse_claims(token: &str) -> Option<JwtClaims> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_url_contains_pkce_state_and_callback() {
        let pkce = generate_pkce();
        let url = authorize_url(&pkce, "stable-state").unwrap();
        assert!(url.starts_with("https://auth.openai.com/oauth/authorize?"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=stable-state"));
        assert!(url.contains("localhost%3A1455%2Fauth%2Fcallback"));
        assert!(pkce.verifier.len() >= 43);
    }

    #[test]
    fn oauth_credentials_never_fall_back_to_the_file_store() {
        let directory = tempfile::tempdir().unwrap();
        let store = CredentialStore::for_test(directory.path().join("credentials.toml"));
        assert!(
            save_tokens(
                &store,
                &CodexTokenBundle {
                    access_token: "access".into(),
                    refresh_token: "refresh".into(),
                    expires_at_ms: 1,
                    account_id: None,
                    email: None,
                }
            )
            .is_err()
        );
        assert!(!directory.path().join("credentials.toml").exists());
    }

    #[tokio::test]
    async fn token_exchange_sends_pkce_and_extracts_account_identity() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"email":"person@example.com","chatgpt_account_id":"account-1"}"#);
        let access = format!("header.{claims}.signature");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let read = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..read]).into_owned();
            let body = serde_json::json!({
                "access_token": access,
                "refresh_token": "refresh-1",
                "expires_in": 3600,
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            request
        });
        let pkce = PkceCodes {
            verifier: "stable-verifier".into(),
            challenge: "stable-challenge".into(),
        };
        let tokens = exchange_code_at(
            &format!("http://{address}/oauth/token"),
            "authorization-code",
            &pkce,
        )
        .await
        .unwrap();
        let request = server.await.unwrap();
        assert!(request.contains("code=authorization-code"));
        assert!(request.contains("code_verifier=stable-verifier"));
        assert_eq!(tokens.email.as_deref(), Some("person@example.com"));
        assert_eq!(tokens.account_id.as_deref(), Some("account-1"));
        assert_eq!(tokens.refresh_token, "refresh-1");
    }
}
