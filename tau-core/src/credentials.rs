//! Credential storage with resolution precedence **env > keyring > file**.
//!
//! - `env`: provider→env-var map (overridable per-provider via
//!   `ProviderConfig::api_key_env`); falls back to `{PROVIDER upper}_API_KEY`.
//! - `keyring`: OS keyring (Secret Service / Keychain / Credential Manager) via
//!   the `keyring` crate, service `"tau"`, account = provider id.
//! - `file`: `~/.config/tau/credentials.toml` (`0600` on unix) used as a
//!   fallback when the keyring is unavailable (e.g. headless/systemd).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const SERVICE: &str = "tau";

/// Known providers and the env var that holds each key. Anything not listed
/// falls back to `{PROVIDER upper}_API_KEY`.
const ENV_MAP: &[(&str, &str)] = &[
    ("anthropic", "ANTHROPIC_API_KEY"),
    ("openai", "OPENAI_API_KEY"),
    ("gemini", "GEMINI_API_KEY"),
    ("google", "GEMINI_API_KEY"),
    ("groq", "GROQ_API_KEY"),
    ("mistral", "MISTRAL_API_KEY"),
    ("deepseek", "DEEPSEEK_API_KEY"),
    ("xai", "XAI_API_KEY"),
    ("openrouter", "OPENROUTER_API_KEY"),
];

/// Providers surfaced by `list` even before any credential is stored.
const KNOWN_PROVIDERS: &[&str] = &[
    "anthropic",
    "openai",
    "gemini",
    "groq",
    "mistral",
    "deepseek",
    "xai",
    "openrouter",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProviderCred {
    api_key: String,
}

type FileStore = BTreeMap<String, ProviderCred>;

/// Credential store. Resolution order: env var → keyring → file fallback.
pub struct CredentialStore {
    file_path: PathBuf,
    use_keyring: bool,
}

impl CredentialStore {
    /// Store at the default file path, keyring enabled.
    pub fn new() -> Result<Self> {
        Ok(Self {
            file_path: credentials_file_path()?,
            use_keyring: true,
        })
    }

    /// Hermetic test constructor: keyring disabled, file at the given path.
    pub fn for_test(file_path: PathBuf) -> Self {
        Self {
            file_path,
            use_keyring: false,
        }
    }

    /// Resolve the API key for a provider, honouring a per-provider env override.
    /// Returns `None` when no source has a key.
    pub fn get(&self, provider: &str, api_key_env: Option<&str>) -> Option<String> {
        if let Some(v) = env_value(provider, api_key_env) {
            return Some(v);
        }
        if self.use_keyring {
            if let Ok(entry) = keyring::Entry::new(SERVICE, provider) {
                if let Ok(pw) = entry.get_password() {
                    return Some(pw);
                }
            }
        }
        self.file_get(provider)
    }

    /// Store a key for a provider. Always writes to the file (persistent,
    /// reliable) and also pushes to the keyring best-effort. The file is the
    /// source of truth; the keyring is a preferred read source when available.
    pub fn set(&self, provider: &str, key: &str) -> Result<()> {
        self.file_set(provider, key)?;
        if self.use_keyring {
            if let Ok(entry) = keyring::Entry::new(SERVICE, provider) {
                let _ = entry.set_password(key);
            }
        }
        Ok(())
    }

    /// Delete a provider's key from every source (best effort).
    pub fn delete(&self, provider: &str) -> Result<()> {
        if self.use_keyring {
            if let Ok(entry) = keyring::Entry::new(SERVICE, provider) {
                let _ = entry.delete_credential();
            }
        }
        self.file_delete(provider)
    }

    /// Provider ids that currently resolve to a credential.
    pub fn list(&self) -> Vec<String> {
        let mut ids: BTreeSet<String> = self.file_keys();
        for p in KNOWN_PROVIDERS {
            ids.insert((*p).to_string());
        }
        ids.into_iter()
            .filter(|p| self.get(p, None).is_some())
            .collect()
    }

    // ---- file fallback ----

    fn read_file(&self) -> FileStore {
        match std::fs::read_to_string(&self.file_path) {
            Ok(s) => toml::from_str(&s).unwrap_or_default(),
            Err(_) => FileStore::default(),
        }
    }

    fn write_file(&self, store: &FileStore) -> Result<()> {
        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let s = toml::to_string_pretty(store).context("serializing credentials")?;
        std::fs::write(&self.file_path, s)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&self.file_path)?.permissions();
            perms.set_mode(0o600);
            std::fs::set_permissions(&self.file_path, perms)?;
        }
        Ok(())
    }

    fn file_get(&self, provider: &str) -> Option<String> {
        self.read_file().get(provider).map(|c| c.api_key.clone())
    }

    fn file_set(&self, provider: &str, key: &str) -> Result<()> {
        let mut store = self.read_file();
        store.insert(
            provider.to_string(),
            ProviderCred {
                api_key: key.to_string(),
            },
        );
        self.write_file(&store)
    }

    fn file_delete(&self, provider: &str) -> Result<()> {
        let mut store = self.read_file();
        store.remove(provider);
        self.write_file(&store)
    }

    fn file_keys(&self) -> BTreeSet<String> {
        self.read_file().keys().cloned().collect()
    }
}

/// Default credentials file path: `$XDG_CONFIG_HOME/tau/credentials.toml`.
pub fn credentials_file_path() -> Result<PathBuf> {
    let base = dirs::config_dir().context("could not determine config directory")?;
    Ok(base.join("tau").join("credentials.toml"))
}

fn env_value(provider: &str, api_key_env: Option<&str>) -> Option<String> {
    let var = api_key_env
        .map(str::to_owned)
        .unwrap_or_else(|| env_var_name(provider));
    std::env::var(&var).ok()
}

fn env_var_name(provider: &str) -> String {
    if let Some((_, v)) = ENV_MAP.iter().find(|(k, _)| *k == provider) {
        return (*v).to_string();
    }
    let upper = provider.to_ascii_uppercase().replace('-', "_");
    format!("{upper}_API_KEY")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> CredentialStore {
        let dir = tempfile::tempdir().unwrap();
        CredentialStore::for_test(dir.path().join("credentials.toml"))
    }

    #[test]
    fn file_set_get_delete_round_trip() {
        let store = temp_store();
        assert!(store.get("anthropic", None).is_none());

        store.set("anthropic", "sk-test-123").unwrap();
        assert_eq!(store.get("anthropic", None).as_deref(), Some("sk-test-123"));
        assert!(store.list().contains(&"anthropic".to_string()));

        store.delete("anthropic").unwrap();
        assert!(store.get("anthropic", None).is_none());
    }

    #[test]
    fn env_overrides_file() {
        let store = temp_store();
        store.set("openai", "file-key").unwrap();
        // env should win when present
        unsafe { std::env::set_var("OPENAI_API_KEY", "env-key") };
        assert_eq!(store.get("openai", None).as_deref(), Some("env-key"));
        unsafe { std::env::remove_var("OPENAI_API_KEY") };
        // back to file
        assert_eq!(store.get("openai", None).as_deref(), Some("file-key"));
    }

    #[test]
    fn api_key_env_override() {
        let store = temp_store();
        unsafe { std::env::set_var("CUSTOM_PROVIDER_KEY", "custom") };
        assert_eq!(
            store
                .get("anything", Some("CUSTOM_PROVIDER_KEY"))
                .as_deref(),
            Some("custom")
        );
        unsafe { std::env::remove_var("CUSTOM_PROVIDER_KEY") };
    }

    #[test]
    fn env_var_name_convention() {
        assert_eq!(env_var_name("anthropic"), "ANTHROPIC_API_KEY");
        assert_eq!(env_var_name("my-provider"), "MY_PROVIDER_API_KEY");
    }
}
