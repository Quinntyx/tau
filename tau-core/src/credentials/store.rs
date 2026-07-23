//! [`CredentialStore`] — resolution precedence **env > keyring > file**.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;

use super::path::credentials_file_path;
use super::provider::Provider;

const SERVICE: &str = "tau";

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

    /// Read a secret exclusively from the operating-system keyring.
    pub fn get_secure(&self, provider: &str) -> Result<Option<String>> {
        anyhow::ensure!(
            self.use_keyring,
            "secure credential storage is unavailable in this environment"
        );
        let entry = keyring::Entry::new(SERVICE, provider)
            .context("opening operating-system credential storage")?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error).context("reading operating-system credential storage"),
        }
    }

    /// Write a secret exclusively to the operating-system keyring.
    pub fn set_secure(&self, provider: &str, secret: &str) -> Result<()> {
        anyhow::ensure!(
            self.use_keyring,
            "secure credential storage is unavailable in this environment"
        );
        keyring::Entry::new(SERVICE, provider)
            .context("opening operating-system credential storage")?
            .set_password(secret)
            .context("writing operating-system credential storage")
    }

    /// Delete a secret exclusively from the operating-system keyring.
    pub fn delete_secure(&self, provider: &str) -> Result<()> {
        anyhow::ensure!(
            self.use_keyring,
            "secure credential storage is unavailable in this environment"
        );
        let entry = keyring::Entry::new(SERVICE, provider)
            .context("opening operating-system credential storage")?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error).context("deleting operating-system credential"),
        }
    }

    /// Resolve the API key for a provider, honouring a per-provider env override.
    /// Returns `None` when no source has a key.
    pub fn get(&self, provider: &str, custom_env: Option<&str>) -> Option<String> {
        if let Some(key) = self.env_lookup(provider, custom_env) {
            return Some(key);
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
        for p in Provider::iter() {
            ids.insert(p.to_string());
        }
        ids.into_iter()
            .filter(|p| self.get(p, None).is_some())
            .collect()
    }

    // ---- env resolution ----

    fn env_lookup(&self, provider: &str, custom_env: Option<&str>) -> Option<String> {
        let var = custom_env
            .map(str::to_owned)
            .unwrap_or_else(|| Provider::env_var_name(provider));
        std::env::var(&var).ok()
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
