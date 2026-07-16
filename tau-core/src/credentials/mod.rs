//! Credential storage with resolution precedence **env > keyring > file**.
//!
//! - `env`: provider→env-var map (overridable per-provider via
//!   `ProviderConfig::api_key_env`); falls back to `{PROVIDER upper}_API_KEY`.
//! - `keyring`: OS keyring (Secret Service / Keychain / Credential Manager) via
//!   the `keyring` crate, service `"tau"`, account = provider id.
//! - `file`: `~/.config/tau/credentials.toml` (`0600` on unix) used as a
//!   fallback when the keyring is unavailable (e.g. headless/systemd).

pub mod path;
pub mod provider;
pub mod store;

pub use path::credentials_file_path;
pub use provider::Provider;
pub use store::CredentialStore;

#[cfg(test)]
mod tests;
