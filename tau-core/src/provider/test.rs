use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use futures::stream;
use rig_core::completion::message::{ToolCall, ToolFunction, ToolResultContent, UserContent};
use rig_core::completion::{CompletionError, CompletionRequest, Message, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::agent::{QuestionAnswer, QuestionBroker, TaskBudget, TaskTier};
use crate::integrations::{LspManager, LspPosition, LspServerConfig, McpManager, McpServerConfig};
use crate::plan::Plan;
use crate::tools::{Tool, ToolContext, ToolDescriptor, ToolRegistry, schema_for};

use super::{TauDelta, TauStream};

pub const ENABLE_ENV: &str = "TAU_ENABLE_TEST_PROVIDER";
pub const FIXTURE_ENV: &str = "TAU_TEST_INTEGRATION_FIXTURE";

#[derive(Clone)]
pub struct TestProvider {
    fixture: Option<PathBuf>,
}

impl TestProvider {
    pub fn from_env() -> Result<Self> {
        anyhow::ensure!(
            std::env::var(ENABLE_ENV).as_deref() == Ok("1"),
            "test provider is disabled; set {ENABLE_ENV}=1 explicitly"
        );
        Ok(Self {
            fixture: std::env::var_os(FIXTURE_ENV).map(PathBuf::from),
        })
    }

    pub fn with_fixture(path: impl Into<PathBuf>) -> Self {
        Self {
            fixture: Some(path.into()),
        }
    }

    pub fn tool_registry(&self, workspace: &Path) -> Result<ToolRegistry> {
        let mut registry = ToolRegistry::with_builtins()?;
        registry.register(TestPlanTool)?;
        registry.register(TestQuestionTool)?;
        registry.register(TestTaskTool)?;
        if let Some(fixture) = &self.fixture {
            registry.register(TestMcpTool::new(fixture))?;
            registry.register(TestLspTool::new(fixture, workspace))?;
        }
        Ok(registry)
    }

    pub async fn stream(&self, request: CompletionRequest) -> Result<TauStream, CompletionError> {
        let scenario = scenario(&request).ok_or_else(|| {
            CompletionError::RequestError(
                "test provider requires test:all-tools, test:filesystem, test:integrations, test:orchestration, or test:error-paths"
                    .into(),
            )
        })?;
        let results = tool_results(&request);
        let mut differences = validate_schemas(&request.tools, scenario, self.fixture.is_some());
        let deltas = if !differences.is_empty() {
            verdict(scenario, 0, differences)
        } else {
            match scenario {
                "test:filesystem" => filesystem_turn(&results, false, false, &mut differences),
                "test:integrations" => integration_turn(&results, &mut differences),
                "test:orchestration" => orchestration_turn(&results, &mut differences),
                "test:all-tools" => filesystem_turn(&results, true, true, &mut differences),
                "test:error-paths" => error_turn(&results, &mut differences),
                _ => unreachable!(),
            }
        };
        Ok(Box::pin(stream::iter(deltas.into_iter().map(Ok))))
    }
}

fn scenario(request: &CompletionRequest) -> Option<&'static str> {
    request
        .chat_history
        .clone()
        .into_iter()
        .find_map(|message| {
            let Message::User { content } = message else {
                return None;
            };
            content.into_iter().find_map(|item| {
                let UserContent::Text(text) = item else {
                    return None;
                };
                match text.text.trim() {
                    "test:filesystem" => Some("test:filesystem"),
                    "test:integrations" => Some("test:integrations"),
                    "test:orchestration" => Some("test:orchestration"),
                    "test:all-tools" => Some("test:all-tools"),
                    "test:error-paths" => Some("test:error-paths"),
                    _ => None,
                }
            })
        })
}

fn tool_results(request: &CompletionRequest) -> BTreeMap<String, Value> {
    let mut results = BTreeMap::new();
    for message in request.chat_history.clone() {
        let Message::User { content } = message else {
            continue;
        };
        for item in content {
            let UserContent::ToolResult(result) = item else {
                continue;
            };
            for content in result.content {
                if let ToolResultContent::Text(text) = content {
                    let value = serde_json::from_str(&text.text)
                        .unwrap_or_else(|_| json!({"invalid_json": text.text}));
                    results.insert(result.id.clone(), value);
                }
            }
        }
    }
    results
}

fn validate_schemas(
    actual: &[ToolDefinition],
    scenario: &str,
    integrations_available: bool,
) -> Vec<String> {
    let mut expected = BTreeMap::new();
    if !matches!(scenario, "test:integrations" | "test:orchestration") {
        for name in ["bash", "edit", "glob", "grep", "list", "read", "write"] {
            expected.insert(name.to_owned(), schema_for(name));
        }
    }
    if matches!(scenario, "test:orchestration" | "test:all-tools") {
        expected.insert("plan".into(), plan_schema());
        expected.insert("question".into(), question_schema());
        expected.insert("task".into(), task_schema());
    }
    if matches!(scenario, "test:integrations" | "test:all-tools") {
        if integrations_available {
            expected.insert("mcp_fixture_echo".into(), mcp_schema());
            expected.insert("lsp_fixture_definition".into(), lsp_schema());
        } else {
            return vec!["integration fixture is not configured".into()];
        }
    }
    let actual = actual
        .iter()
        .map(|tool| (tool.name.clone(), tool.parameters.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut differences = Vec::new();
    for (name, schema) in expected {
        match actual.get(&name) {
            Some(value) if value == &schema => {}
            Some(value) => differences.push(format!("schema mismatch for {name}: {value}")),
            None => differences.push(format!("missing tool schema: {name}")),
        }
    }
    differences
}

fn filesystem_turn(
    results: &BTreeMap<String, Value>,
    include_integrations: bool,
    include_orchestration: bool,
    differences: &mut Vec<String>,
) -> Vec<TauDelta> {
    if !results.contains_key("write") {
        let mut calls = vec![
            call(
                "write",
                "write",
                json!({"path":"generated.txt","content":"alpha\nbeta\n"}),
            ),
            call("list", "list", json!({"path":"."})),
            call("read", "read", json!({"file_path":"generated.txt"})),
            call("glob", "glob", json!({"pattern":"**/*.txt","path":"."})),
            call(
                "grep",
                "grep",
                json!({"pattern":"beta","path":".","include":"*.txt"}),
            ),
            call("bash", "bash", json!({"command":"printf 'stable\\n'"})),
        ];
        if include_integrations {
            calls.extend(integration_calls());
        }
        if include_orchestration {
            calls.extend(orchestration_calls());
        }
        return calls;
    }
    validate_stage_one(
        results,
        include_integrations,
        include_orchestration,
        differences,
    );
    if !results.contains_key("edit") {
        let hash_length = crate::tools::hashline::adaptive_hash_length(2);
        let reference = format!(
            "1#{}",
            crate::tools::hashline::line_hash("alpha", hash_length)
        );
        let rev = results
            .get("read")
            .and_then(|value| value.get("rev"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        return vec![call(
            "edit",
            "edit",
            json!({"path":"generated.txt","content":"ALPHA","ref":reference,"fileRev":rev}),
        )];
    }
    validate_object_fields(
        results,
        "edit",
        &["path", "changed", "snapshot_id"],
        differences,
    );
    if !results.contains_key("final-read") {
        return vec![call(
            "final-read",
            "read",
            json!({"file_path":"generated.txt"}),
        )];
    }
    let content = results
        .get("final-read")
        .and_then(|value| value.get("content"))
        .and_then(Value::as_str);
    if content != Some("ALPHA\nbeta") {
        differences.push(format!("final read content mismatch: {content:?}"));
    }
    let count = if include_integrations { 13 } else { 8 };
    verdict(
        if include_integrations {
            "test:all-tools"
        } else {
            "test:filesystem"
        },
        count,
        std::mem::take(differences),
    )
}

fn integration_turn(
    results: &BTreeMap<String, Value>,
    differences: &mut Vec<String>,
) -> Vec<TauDelta> {
    if !results.contains_key("mcp") {
        return integration_calls();
    }
    validate_integrations(results, differences);
    verdict("test:integrations", 2, std::mem::take(differences))
}

fn orchestration_turn(
    results: &BTreeMap<String, Value>,
    differences: &mut Vec<String>,
) -> Vec<TauDelta> {
    if !results.contains_key("plan") {
        return orchestration_calls();
    }
    validate_orchestration(results, differences);
    verdict("test:orchestration", 3, std::mem::take(differences))
}

fn error_turn(results: &BTreeMap<String, Value>, differences: &mut Vec<String>) -> Vec<TauDelta> {
    if !results.contains_key("escape") {
        return vec![
            call("escape", "read", json!({"file_path":"../escape.txt"})),
            call("unknown", "missing_tool", json!({})),
            call("nonzero", "bash", json!({"command":"exit 7"})),
        ];
    }
    for id in ["escape", "unknown"] {
        if results
            .get(id)
            .and_then(|value| value.get("error"))
            .and_then(Value::as_str)
            .is_none()
        {
            differences.push(format!("{id} did not return a structured error"));
        }
    }
    if results
        .get("nonzero")
        .and_then(|value| value.get("exit_code"))
        .and_then(Value::as_i64)
        != Some(7)
    {
        differences.push("nonzero bash exit code mismatch".into());
    }
    verdict("test:error-paths", 3, std::mem::take(differences))
}

fn validate_stage_one(
    results: &BTreeMap<String, Value>,
    integrations: bool,
    orchestration: bool,
    differences: &mut Vec<String>,
) {
    validate_object_fields(
        results,
        "write",
        &["path", "existed", "bytes", "snapshot_id"],
        differences,
    );
    validate_object_fields(
        results,
        "list",
        &["path", "entries", "truncated"],
        differences,
    );
    validate_object_fields(
        results,
        "read",
        &[
            "type",
            "path",
            "rev",
            "content",
            "line_start",
            "line_end",
            "total_lines",
            "truncated",
        ],
        differences,
    );
    validate_object_fields(
        results,
        "glob",
        &["root", "entries", "truncated"],
        differences,
    );
    validate_object_fields(results, "grep", &["matches", "truncated"], differences);
    validate_object_fields(
        results,
        "bash",
        &[
            "command",
            "cwd",
            "exit_code",
            "stdout",
            "stderr",
            "timed_out",
            "truncated",
            "classification",
            "snapshot_id",
        ],
        differences,
    );
    if results
        .get("read")
        .and_then(|value| value.get("content"))
        .and_then(Value::as_str)
        != Some("alpha\nbeta")
    {
        differences.push("read content mismatch".into());
    }
    if results
        .get("bash")
        .and_then(|value| value.get("stdout"))
        .and_then(Value::as_str)
        != Some("stable")
    {
        differences.push("bash stdout mismatch".into());
    }
    if integrations {
        validate_integrations(results, differences);
    }
    if orchestration {
        validate_orchestration(results, differences);
    }
}

fn validate_integrations(results: &BTreeMap<String, Value>, differences: &mut Vec<String>) {
    if results
        .get("mcp")
        .and_then(|value| value.get("content"))
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|value| value.get("text"))
        .and_then(Value::as_str)
        != Some("stable")
    {
        differences.push("MCP echo result mismatch".into());
    }
    if !results.get("lsp").is_some_and(Value::is_array) {
        differences.push("LSP definition result is not an array".into());
    }
}

fn validate_orchestration(results: &BTreeMap<String, Value>, differences: &mut Vec<String>) {
    if results
        .get("plan")
        .and_then(|value| value.get("airtight"))
        .and_then(Value::as_bool)
        != Some(true)
    {
        differences.push("plan did not produce an airtight current step".into());
    }
    if results
        .get("question")
        .and_then(|value| value.get("answer"))
        .and_then(Value::as_str)
        != Some("stable")
    {
        differences.push("question broker answer mismatch".into());
    }
    if results
        .get("task")
        .and_then(|value| value.get("child_depth"))
        .and_then(Value::as_u64)
        != Some(1)
    {
        differences.push("task budget child depth mismatch".into());
    }
}

fn validate_object_fields(
    results: &BTreeMap<String, Value>,
    id: &str,
    expected: &[&str],
    differences: &mut Vec<String>,
) {
    let Some(object) = results.get(id).and_then(Value::as_object) else {
        differences.push(format!("{id} result is not an object"));
        return;
    };
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    if actual != expected {
        differences.push(format!(
            "{id} fields differ: expected {expected:?}, got {actual:?}"
        ));
    }
}

fn integration_calls() -> Vec<TauDelta> {
    vec![
        call("mcp", "mcp_fixture_echo", json!({"value":"stable"})),
        call(
            "lsp",
            "lsp_fixture_definition",
            json!({"file_path":"fixture.rs","line":0,"character":0}),
        ),
    ]
}

fn orchestration_calls() -> Vec<TauDelta> {
    vec![
        call("plan", "plan", json!({"title":"stable plan"})),
        call(
            "question",
            "question",
            json!({"question":"stable?","answer":"stable"}),
        ),
        call("task", "task", json!({"tier":"low"})),
    ]
}

fn call(id: &str, name: &str, arguments: Value) -> TauDelta {
    TauDelta::ToolCall(ToolCall::new(
        id.into(),
        ToolFunction::new(name.into(), arguments),
    ))
}

fn verdict(scenario: &str, tool_calls: usize, differences: Vec<String>) -> Vec<TauDelta> {
    vec![TauDelta::Text(
        json!({
            "scenario": scenario,
            "status": if differences.is_empty() { "passed" } else { "failed" },
            "tool_calls": tool_calls,
            "differences": differences,
        })
        .to_string(),
    )]
}

fn mcp_schema() -> Value {
    json!({"type":"object","properties":{"value":{"type":"string"}},"required":["value"],"additionalProperties":false})
}

fn lsp_schema() -> Value {
    json!({"type":"object","properties":{"file_path":{"type":"string"},"line":{"type":"integer","minimum":0},"character":{"type":"integer","minimum":0}},"required":["file_path","line","character"],"additionalProperties":false})
}

fn plan_schema() -> Value {
    json!({"type":"object","properties":{"title":{"type":"string"}},"required":["title"],"additionalProperties":false})
}

fn question_schema() -> Value {
    json!({"type":"object","properties":{"question":{"type":"string"},"answer":{"type":"string"}},"required":["question","answer"],"additionalProperties":false})
}

fn task_schema() -> Value {
    json!({"type":"object","properties":{"tier":{"type":"string","enum":["low"]}},"required":["tier"],"additionalProperties":false})
}

#[derive(Clone)]
struct TestPlanTool;

#[derive(Deserialize)]
struct TestPlanInput {
    title: String,
}

impl Tool for TestPlanTool {
    type Input = TestPlanInput;
    type Output = Value;

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "plan".into(),
            description: "Environment-gated deterministic plan lifecycle".into(),
        }
    }

    fn schema(&self) -> Value {
        plan_schema()
    }

    fn execute(
        &self,
        input: Self::Input,
        _: &ToolContext,
    ) -> Result<Self::Output, crate::tools::ToolError> {
        let mut plan = Plan::new(input.title, "test provider contract");
        let step = plan.add_step("execute");
        plan.add_item(step, "verify");
        plan.airtight_step(step);
        Ok(json!({
            "current_step": plan.current_step,
            "airtight": plan.current_is_airtight(),
            "revision": plan.revision,
        }))
    }

    fn render(&self, output: &Self::Output) -> String {
        output.to_string()
    }
}

#[derive(Clone)]
struct TestQuestionTool;

#[derive(Deserialize)]
struct TestQuestionInput {
    question: String,
    answer: String,
}

impl Tool for TestQuestionTool {
    type Input = TestQuestionInput;
    type Output = Value;

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "question".into(),
            description: "Environment-gated deterministic question lifecycle".into(),
        }
    }

    fn schema(&self) -> Value {
        question_schema()
    }

    fn execute(
        &self,
        input: Self::Input,
        _: &ToolContext,
    ) -> Result<Self::Output, crate::tools::ToolError> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let broker = QuestionBroker::default();
                let item = broker.ask(input.question, vec![input.answer.clone()]).await;
                broker
                    .answer(QuestionAnswer {
                        id: item.id.clone(),
                        answer: input.answer,
                    })
                    .await;
                let answer = broker.wait(&item.id).await;
                Ok(json!({"answer":answer,"pending":broker.pending().await.len()}))
            })
        })
    }

    fn render(&self, output: &Self::Output) -> String {
        output.to_string()
    }
}

#[derive(Clone)]
struct TestTaskTool;

#[derive(Deserialize)]
struct TestTaskInput {
    tier: String,
}

impl Tool for TestTaskTool {
    type Input = TestTaskInput;
    type Output = Value;

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "task".into(),
            description: "Environment-gated deterministic recursive task budget".into(),
        }
    }

    fn schema(&self) -> Value {
        task_schema()
    }

    fn execute(
        &self,
        input: Self::Input,
        _: &ToolContext,
    ) -> Result<Self::Output, crate::tools::ToolError> {
        if input.tier != "low" {
            return Err(crate::tools::ToolError::InvalidInput(
                "test task tier must be low".into(),
            ));
        }
        let budget = TaskBudget::new(TaskTier::Low);
        let child = budget
            .child()
            .map_err(|error| crate::tools::ToolError::Integration(format!("{error:?}")))?;
        let grandchild_error = match child.child() {
            Ok(_) => "none".into(),
            Err(error) => format!("{error:?}"),
        };
        Ok(json!({
            "child_depth": child.depth(),
            "grandchild_error": grandchild_error,
        }))
    }

    fn render(&self, output: &Self::Output) -> String {
        output.to_string()
    }
}

#[derive(Clone)]
struct TestMcpTool {
    manager: Arc<tokio::sync::Mutex<McpManager>>,
}

#[derive(Deserialize)]
struct TestMcpInput {
    value: String,
}

impl TestMcpTool {
    fn new(fixture: &Path) -> Self {
        let mut manager = McpManager::new();
        manager.register(
            "fixture",
            McpServerConfig {
                command: "python3".into(),
                args: vec![fixture.to_string_lossy().into_owned()],
                timeout_ms: 5_000,
                env: BTreeMap::new(),
                cwd: None,
                max_restarts: 1,
            },
        );
        Self {
            manager: Arc::new(tokio::sync::Mutex::new(manager)),
        }
    }
}

impl Tool for TestMcpTool {
    type Input = TestMcpInput;
    type Output = Value;

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "mcp_fixture_echo".into(),
            description: "Environment-gated deterministic MCP fixture call".into(),
        }
    }

    fn schema(&self) -> Value {
        mcp_schema()
    }

    fn execute(
        &self,
        input: Self::Input,
        _: &ToolContext,
    ) -> Result<Self::Output, crate::tools::ToolError> {
        let manager = Arc::clone(&self.manager);
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                manager
                    .lock()
                    .await
                    .call_tool("fixture", "fixture_echo", json!({"value": input.value}))
                    .await
                    .map_err(|error| crate::tools::ToolError::Integration(error.to_string()))
            })
        })
    }

    fn render(&self, output: &Self::Output) -> String {
        output.to_string()
    }
}

#[derive(Clone)]
struct TestLspTool {
    manager: Arc<tokio::sync::Mutex<LspManager>>,
}

#[derive(Deserialize)]
struct TestLspInput {
    file_path: PathBuf,
    line: u32,
    character: u32,
}

impl TestLspTool {
    fn new(fixture: &Path, workspace: &Path) -> Self {
        let mut manager = LspManager::new();
        manager.register(
            "fixture",
            LspServerConfig {
                command: "python3".into(),
                args: vec![fixture.to_string_lossy().into_owned()],
                root: workspace.to_path_buf(),
                language_id: "rust".into(),
                timeout_ms: 5_000,
            },
        );
        Self {
            manager: Arc::new(tokio::sync::Mutex::new(manager)),
        }
    }
}

impl Tool for TestLspTool {
    type Input = TestLspInput;
    type Output = Value;

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "lsp_fixture_definition".into(),
            description: "Environment-gated deterministic LSP definition request".into(),
        }
    }

    fn schema(&self) -> Value {
        lsp_schema()
    }

    fn execute(
        &self,
        input: Self::Input,
        context: &ToolContext,
    ) -> Result<Self::Output, crate::tools::ToolError> {
        let manager = Arc::clone(&self.manager);
        let path = context.cwd.join(input.file_path);
        let uri = format!("file://{}", path.to_string_lossy());
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let mut manager = manager.lock().await;
                let client = manager
                    .client("fixture")
                    .await
                    .map_err(|error| crate::tools::ToolError::Integration(error.to_string()))?;
                client
                    .definition(
                        &uri,
                        LspPosition {
                            line: input.line,
                            character: input.character,
                        },
                    )
                    .await
                    .map_err(|error| crate::tools::ToolError::Integration(error.to_string()))
                    .and_then(|locations| {
                        serde_json::to_value(locations).map_err(|error| {
                            crate::tools::ToolError::Serialization(error.to_string())
                        })
                    })
            })
        })
    }

    fn render(&self, output: &Self::Output) -> String {
        output.to_string()
    }
}
