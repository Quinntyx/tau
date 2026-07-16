//! Integration tests for the credentials module.

use super::store::CredentialStore;

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
    unsafe { std::env::set_var("OPENAI_API_KEY", "env-key") };
    assert_eq!(store.get("openai", None).as_deref(), Some("env-key"));
    unsafe { std::env::remove_var("OPENAI_API_KEY") };
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
