//! Typed provider-account protocol shapes.
use serde::{Deserialize, Serialize};

pub const METHOD_AUTH_STATUS: &str = "auth.status";
pub const METHOD_AUTH_BEGIN: &str = "auth.begin";
pub const METHOD_AUTH_CANCEL: &str = "auth.cancel";
pub const METHOD_AUTH_LOGOUT: &str = "auth.logout";

pub const OPENAI_CODEX_PROVIDER: &str = "openai-codex";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthProviderParams {
    pub provider: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AuthState {
    SignedOut,
    Pending {
        authorize_url: String,
    },
    SignedIn {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        email: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account_id: Option<String>,
    },
    Failed {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthStatusResult {
    pub provider: String,
    pub status: AuthState,
}

pub type AuthBeginResult = AuthStatusResult;
pub type AuthCancelResult = AuthStatusResult;
pub type AuthLogoutResult = AuthStatusResult;
