//! Integration tests for the config module.

use std::path::Path;

use super::*;

#[test]
fn missing_file_is_default() {
    let cfg = Config::load_from(Path::new("/nonexistent/tau/config.kdl")).unwrap();
    assert!(cfg.model.is_none());
    assert!(cfg.providers.is_empty());
}

#[test]
fn parses_minimal() {
    let text = r#"model "claude-opus"
default_agent "plan"
provider "anthropic" {
  api_base "https://custom.example.com"
  api_key_env "MY_ANTHROPIC_KEY"
}"#;
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.kdl");
    std::fs::write(&p, text).unwrap();
    let cfg = Config::load_from(&p).unwrap();
    assert_eq!(cfg.model.as_deref(), Some("claude-opus"));
    assert_eq!(cfg.default_agent.as_deref(), Some("plan"));
    let anthropic = cfg.providers.get("anthropic").unwrap();
    assert_eq!(
        anthropic.api_base.as_deref(),
        Some("https://custom.example.com")
    );
    assert_eq!(anthropic.api_key_env.as_deref(), Some("MY_ANTHROPIC_KEY"));
}

#[test]
fn unknown_fields_ignored() {
    let text = "model \"x\"\nfuture_field \"ignored\"";
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.kdl");
    std::fs::write(&p, text).unwrap();
    assert!(Config::load_from(&p).is_err());
}

#[test]
fn toml_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("config.toml");
    std::fs::write(&p, "model = 'x'").unwrap();
    assert!(Config::load_from(&p).is_err());
}
