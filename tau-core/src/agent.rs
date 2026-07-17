//! M6 model-turn orchestration boundary.
//!
//! The first runner delegates a turn to the normalized provider stream. Tool
//! execution and hook-driven multi-turn loops can be added behind this stable
//! boundary without changing clients.

use rig_core::completion::{CompletionError, CompletionRequest, ToolDefinition};

use crate::provider::{Provider, TauStream};
use crate::tools::ToolRegistry;

#[derive(Clone)]
pub struct AgentRunner {
    provider: Provider,
    tools: ToolRegistry,
}

impl AgentRunner {
    pub fn new(provider: Provider) -> Self {
        Self {
            provider,
            tools: ToolRegistry::with_builtins().unwrap_or_default(),
        }
    }

    pub fn with_tools(mut self, tools: ToolRegistry) -> Self {
        self.tools = tools;
        self
    }

    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .descriptors()
            .into_iter()
            .map(|descriptor| ToolDefinition {
                name: descriptor.name,
                description: descriptor.description,
                parameters: serde_json::json!({"type": "object"}),
            })
            .collect()
    }

    pub async fn stream(&self, request: CompletionRequest) -> Result<TauStream, CompletionError> {
        self.provider.stream(request).await
    }
}
