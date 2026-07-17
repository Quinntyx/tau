//! Managed stdio integrations.  MCP and LSP deliberately share only the
//! framing transport; protocol lifecycle and models are kept typed here.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};

/// The rmcp transport used by daemon-owned child processes.
pub type RmcpStdioTransport = rmcp::transport::TokioChildProcess;

pub struct StdioRpc {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    timeout: Duration,
    notifications: Vec<Value>,
}
impl StdioRpc {
    pub async fn spawn(command: &str, args: &[String]) -> Result<Self> {
        Self::spawn_with_timeout(command, args, Duration::from_secs(30)).await
    }
    pub async fn spawn_with_timeout(
        command: &str,
        args: &[String],
        timeout: Duration,
    ) -> Result<Self> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("starting stdio process `{command}`"))?;
        let stdin = child.stdin.take().context("stdio process has no stdin")?;
        let stdout = child.stdout.take().context("stdio process has no stdout")?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            timeout,
            notifications: Vec::new(),
        })
    }
    pub async fn request<P: Serialize>(&mut self, method: &str, params: P) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(serde_json::json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;
        loop {
            let value = timeout(self.timeout, self.receive())
                .await
                .context("stdio request timed out")??;
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                if value.get("method").is_some() {
                    self.notifications.push(value);
                }
                continue;
            }
            if let Some(error) = value.get("error") {
                bail!("stdio JSON-RPC error: {error}");
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
    }
    pub fn take_notifications(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.notifications)
    }
    pub async fn notification<P: Serialize>(&mut self, method: &str, params: P) -> Result<()> {
        self.send(serde_json::json!({"jsonrpc":"2.0","method":method,"params":params}))
            .await
    }
    async fn send(&mut self, value: Value) -> Result<()> {
        let body = serde_json::to_vec(&value)?;
        self.stdin
            .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
            .await?;
        self.stdin.write_all(&body).await?;
        self.stdin.flush().await?;
        Ok(())
    }
    async fn receive(&mut self) -> Result<Value> {
        let mut length = None;
        loop {
            let mut line = String::new();
            if self.stdout.read_line(&mut line).await? == 0 {
                bail!("stdio process exited");
            }
            if line == "\r\n" || line == "\n" {
                break;
            }
            if let Some((n, v)) = line.split_once(':')
                && n.eq_ignore_ascii_case("content-length")
            {
                length = Some(v.trim().parse::<usize>()?);
            }
        }
        let mut body = vec![0; length.context("missing Content-Length")?];
        self.stdout.read_exact(&mut body).await?;
        Ok(serde_json::from_slice(&body)?)
    }
    pub async fn shutdown(&mut self) -> Result<()> {
        self.stdin.shutdown().await?;
        let _ = timeout(Duration::from_secs(2), self.child.wait()).await;
        Ok(())
    }
}
impl Drop for StdioRpc {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}
fn default_timeout() -> u64 {
    30_000
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub input_schema: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPrompt {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}
pub struct McpClient {
    config: McpServerConfig,
    rpc: StdioRpc,
}
impl McpClient {
    pub async fn connect(config: McpServerConfig) -> Result<Self> {
        let rpc = StdioRpc::spawn_with_timeout(
            &config.command,
            &config.args,
            Duration::from_millis(config.timeout_ms),
        )
        .await?;
        let mut this = Self { config, rpc };
        this.rpc.request("initialize", serde_json::json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"tau","version":"0.1"}})).await?;
        this.rpc
            .notification("notifications/initialized", serde_json::json!({}))
            .await?;
        Ok(this)
    }
    pub async fn tools(&mut self) -> Result<Vec<McpTool>> {
        Ok(self
            .rpc
            .request("tools/list", serde_json::json!({}))
            .await?
            .get("tools")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default())
    }
    pub async fn prompts(&mut self) -> Result<Vec<McpPrompt>> {
        Ok(self
            .rpc
            .request("prompts/list", serde_json::json!({}))
            .await?
            .get("prompts")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default())
    }
    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        self.rpc
            .request(
                "tools/call",
                serde_json::json!({"name":name,"arguments":arguments}),
            )
            .await
    }
    pub async fn get_prompt(&mut self, name: &str, arguments: Value) -> Result<Value> {
        self.rpc
            .request(
                "prompts/get",
                serde_json::json!({"name":name,"arguments":arguments}),
            )
            .await
    }
    pub async fn restart(&mut self) -> Result<()> {
        self.rpc.shutdown().await?;
        self.rpc = StdioRpc::spawn_with_timeout(
            &self.config.command,
            &self.config.args,
            Duration::from_millis(self.config.timeout_ms),
        )
        .await?;
        self.rpc.request("initialize", serde_json::json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"tau","version":"0.1"}})).await?;
        self.rpc
            .notification("notifications/initialized", serde_json::json!({}))
            .await
    }
}
pub struct McpManager {
    servers: std::collections::HashMap<String, McpServerConfig>,
    clients: std::collections::HashMap<String, McpClient>,
}
impl McpManager {
    pub fn new() -> Self {
        Self {
            servers: Default::default(),
            clients: Default::default(),
        }
    }
    pub fn register(&mut self, name: impl Into<String>, config: McpServerConfig) {
        self.servers.insert(name.into(), config);
    }
    pub async fn client(&mut self, name: &str) -> Result<&mut McpClient> {
        if !self.clients.contains_key(name) {
            let c = self
                .servers
                .get(name)
                .cloned()
                .context("unknown MCP server")?;
            self.clients
                .insert(name.to_owned(), McpClient::connect(c).await?);
        }
        self.clients.get_mut(name).context("MCP client disappeared")
    }
}
impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub root: PathBuf,
    #[serde(default)]
    pub language_id: String,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LspPosition {
    pub line: u32,
    pub character: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LspRange {
    pub start: LspPosition,
    pub end: LspPosition,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspDiagnostic {
    pub range: LspRange,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub severity: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspLocation {
    pub uri: String,
    pub range: LspRange,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspTextEdit {
    pub range: LspRange,
    pub new_text: String,
}
pub struct LspClient {
    config: LspServerConfig,
    rpc: StdioRpc,
    diagnostics: std::collections::HashMap<String, Vec<LspDiagnostic>>,
    open_documents: std::collections::HashMap<String, (String, String, i32)>,
}
impl LspClient {
    pub async fn connect(config: LspServerConfig) -> Result<Self> {
        let rpc = StdioRpc::spawn_with_timeout(
            &config.command,
            &config.args,
            Duration::from_millis(config.timeout_ms),
        )
        .await?;
        let mut c = Self {
            config,
            rpc,
            diagnostics: Default::default(),
            open_documents: Default::default(),
        };
        c.initialize().await?;
        Ok(c)
    }
    async fn initialize(&mut self) -> Result<()> {
        let root = format!("file://{}", self.config.root.display());
        self.rpc
            .request(
                "initialize",
                serde_json::json!({"processId":null,"rootUri":root,"capabilities":{}}),
            )
            .await?;
        self.rpc
            .notification("initialized", serde_json::json!({}))
            .await
    }
    pub async fn diagnostics(&self, uri: &str) -> &[LspDiagnostic] {
        self.diagnostics.get(uri).map(Vec::as_slice).unwrap_or(&[])
    }
    /// Drain server notifications received while servicing a request.
    /// Diagnostics are published asynchronously by LSP servers, so callers
    /// must dispatch them rather than silently dropping them.
    pub fn dispatch_notifications(&mut self) {
        for notification in self.rpc.take_notifications() {
            if notification.get("method").and_then(Value::as_str)
                != Some("textDocument/publishDiagnostics")
            {
                continue;
            }
            let Some(params) = notification.get("params") else {
                continue;
            };
            let (Some(uri), Some(items)) = (
                params.get("uri").and_then(Value::as_str),
                params.get("diagnostics"),
            ) else {
                continue;
            };
            if let Ok(items) = serde_json::from_value(items.clone()) {
                self.diagnostics.insert(uri.to_owned(), items);
            }
        }
    }
    pub async fn request_locations(
        &mut self,
        method: &str,
        uri: &str,
        position: LspPosition,
    ) -> Result<Vec<LspLocation>> {
        Ok(serde_json::from_value(
            self.rpc
                .request(
                    method,
                    serde_json::json!({"textDocument":{"uri":uri},"position":position}),
                )
                .await?,
        )
        .unwrap_or_default())
    }
    pub async fn definition(
        &mut self,
        uri: &str,
        position: LspPosition,
    ) -> Result<Vec<LspLocation>> {
        self.request_locations("textDocument/definition", uri, position)
            .await
    }
    pub async fn references(
        &mut self,
        uri: &str,
        position: LspPosition,
    ) -> Result<Vec<LspLocation>> {
        self.request_locations("textDocument/references", uri, position)
            .await
    }
    pub async fn declaration(
        &mut self,
        uri: &str,
        position: LspPosition,
    ) -> Result<Vec<LspLocation>> {
        self.request_locations("textDocument/declaration", uri, position)
            .await
    }
    pub async fn implementation(
        &mut self,
        uri: &str,
        position: LspPosition,
    ) -> Result<Vec<LspLocation>> {
        self.request_locations("textDocument/implementation", uri, position)
            .await
    }
    pub async fn type_definition(
        &mut self,
        uri: &str,
        position: LspPosition,
    ) -> Result<Vec<LspLocation>> {
        self.request_locations("textDocument/typeDefinition", uri, position)
            .await
    }
    pub async fn document_symbols(&mut self, uri: &str) -> Result<Value> {
        self.rpc
            .request(
                "textDocument/documentSymbol",
                serde_json::json!({"textDocument":{"uri":uri}}),
            )
            .await
    }
    pub async fn hover(&mut self, uri: &str, position: LspPosition) -> Result<Value> {
        self.rpc
            .request(
                "textDocument/hover",
                serde_json::json!({"textDocument":{"uri":uri},"position":position}),
            )
            .await
    }
    pub async fn completion(&mut self, uri: &str, position: LspPosition) -> Result<Value> {
        self.rpc
            .request(
                "textDocument/completion",
                serde_json::json!({"textDocument":{"uri":uri},"position":position}),
            )
            .await
    }
    pub async fn code_actions(&mut self, uri: &str, range: LspRange) -> Result<Value> {
        self.rpc.request("textDocument/codeAction", serde_json::json!({"textDocument":{"uri":uri},"range":range,"context":{"diagnostics":[]}})).await
    }
    pub async fn formatting(&mut self, uri: &str) -> Result<Vec<LspTextEdit>> {
        Ok(serde_json::from_value(self.rpc.request("textDocument/formatting", serde_json::json!({"textDocument":{"uri":uri},"options":{"tabSize":4,"insertSpaces":true}})).await?).unwrap_or_default())
    }
    pub async fn rename(
        &mut self,
        uri: &str,
        position: LspPosition,
        new_name: &str,
    ) -> Result<Value> {
        self.rpc.request("textDocument/rename", serde_json::json!({"textDocument":{"uri":uri},"position":position,"newName":new_name})).await
    }
    pub async fn open(
        &mut self,
        uri: &str,
        text: &str,
        language_id: &str,
        version: i32,
    ) -> Result<()> {
        self.open_documents.insert(
            uri.to_owned(),
            (text.to_owned(), language_id.to_owned(), version),
        );
        self.rpc.notification("textDocument/didOpen", serde_json::json!({"textDocument":{"uri":uri,"languageId":language_id,"version":version,"text":text}})).await
    }
    pub async fn change(&mut self, uri: &str, text: &str, version: i32) -> Result<()> {
        if let Some(document) = self.open_documents.get_mut(uri) {
            document.0 = text.to_owned();
            document.2 = version;
        }
        self.rpc.notification("textDocument/didChange", serde_json::json!({"textDocument":{"uri":uri,"version":version},"contentChanges":[{"text":text}]})).await
    }
    pub async fn close(&mut self, uri: &str) -> Result<()> {
        self.open_documents.remove(uri);
        self.rpc
            .notification(
                "textDocument/didClose",
                serde_json::json!({"textDocument":{"uri":uri}}),
            )
            .await
    }
    pub fn record_diagnostics(&mut self, uri: impl Into<String>, diagnostics: Vec<LspDiagnostic>) {
        self.diagnostics.insert(uri.into(), diagnostics);
    }
    pub async fn apply_edits(&mut self, uri: &str, edits: Vec<LspTextEdit>) -> Result<Value> {
        self.rpc
            .request(
                "workspace/applyEdit",
                serde_json::json!({"edit":{"changes":{uri:edits}}}),
            )
            .await
    }
    pub async fn restart(&mut self) -> Result<()> {
        self.rpc.shutdown().await?;
        self.rpc = StdioRpc::spawn_with_timeout(
            &self.config.command,
            &self.config.args,
            Duration::from_millis(self.config.timeout_ms),
        )
        .await?;
        self.initialize().await?;
        for (uri, (text, language_id, version)) in self.open_documents.clone() {
            self.rpc.notification("textDocument/didOpen", serde_json::json!({"textDocument":{"uri":uri,"languageId":language_id,"version":version,"text":text}})).await?;
        }
        Ok(())
    }
}
pub struct LspManager {
    configs: std::collections::HashMap<String, LspServerConfig>,
    clients: std::collections::HashMap<String, LspClient>,
}
impl LspManager {
    pub fn new() -> Self {
        Self {
            configs: Default::default(),
            clients: Default::default(),
        }
    }
    pub fn register(&mut self, language: impl Into<String>, config: LspServerConfig) {
        self.configs.insert(language.into(), config);
    }
    pub async fn client(&mut self, language: &str) -> Result<&mut LspClient> {
        if !self.clients.contains_key(language) {
            let c = self
                .configs
                .get(language)
                .cloned()
                .context("unknown language server")?;
            self.clients
                .insert(language.to_owned(), LspClient::connect(c).await?);
        }
        self.clients
            .get_mut(language)
            .context("LSP client disappeared")
    }
}
impl Default for LspManager {
    fn default() -> Self {
        Self::new()
    }
}
