use std::collections::BTreeMap;

use tau_core::integrations::{McpClient, McpEvent, McpManager, McpServerConfig};

#[tokio::test]
async fn rmcp_stdio_fixture_supports_tools_and_calls() {
    let config = McpServerConfig {
        command: "python3".into(),
        args: vec![format!(
            "{}/tests/fixtures/mcp_server.py",
            env!("CARGO_MANIFEST_DIR")
        )],
        timeout_ms: 5_000,
        env: BTreeMap::new(),
        cwd: None,
        max_restarts: 1,
    };
    let mut client = McpClient::connect(config).await.unwrap();
    let tools = client.tools().await.unwrap();
    assert_eq!(tools[0].name, "fixture_echo");
    let result = client
        .call_tool("fixture_echo", serde_json::json!({"value": "stable"}))
        .await
        .unwrap();
    assert_eq!(result["content"][0]["text"], "stable");
}

fn fixture_config() -> McpServerConfig {
    McpServerConfig {
        command: "python3".into(),
        args: vec![format!(
            "{}/tests/fixtures/mcp_server.py",
            env!("CARGO_MANIFEST_DIR")
        )],
        timeout_ms: 5_000,
        env: [("TAU_FIXTURE_VALUE".into(), "configured".into())]
            .into_iter()
            .collect(),
        cwd: None,
        max_restarts: 1,
    }
}

#[tokio::test]
async fn rmcp_fixture_supports_prompts_and_configured_environment() {
    let mut client = McpClient::connect(fixture_config()).await.unwrap();
    let prompts = client.prompts().await.unwrap();
    assert_eq!(prompts[0].name, "fixture_prompt");
    let prompt = client
        .get_prompt("fixture_prompt", serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(prompt["messages"][0]["content"]["text"], "fixture prompt");
    let env = client
        .call_tool("fixture_env", serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(env["content"][0]["text"], "configured");
}

#[tokio::test]
async fn manager_emits_typed_discovery_and_bounded_restart_events() {
    let mut manager = McpManager::new();
    manager.register("fixture", fixture_config());
    let tools = manager.dynamic_tools("fixture").await.unwrap();
    assert!(tools.iter().any(|tool| tool.name == "fixture_echo"));
    assert!(
        manager
            .take_events()
            .iter()
            .any(|event| matches!(event, McpEvent::ToolDiscovered { .. }))
    );

    let result = manager
        .call_tool("fixture", "fixture_crash", serde_json::json!({}))
        .await;
    assert!(result.is_err());
    let events = manager.take_events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, McpEvent::ServerRestarted { attempt: 1, .. }))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, McpEvent::ToolCallFailed { .. }))
    );
}
