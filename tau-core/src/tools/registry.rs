use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Serialize, de::DeserializeOwned};

use super::error::ToolError;
use super::types::ToolContext;

#[derive(Debug, Clone, Serialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolResult {
    pub output: serde_json::Value,
    pub rendered: String,
}

pub trait Tool: Send + Sync + 'static {
    type Input: DeserializeOwned;
    type Output: Serialize;

    fn descriptor(&self) -> ToolDescriptor;
    fn execute(&self, input: Self::Input, context: &ToolContext)
    -> Result<Self::Output, ToolError>;
    fn render(&self, output: &Self::Output) -> String;
}

trait ErasedTool: Send + Sync {
    fn descriptor(&self) -> ToolDescriptor;
    fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError>;
}

impl<T> ErasedTool for T
where
    T: Tool,
{
    fn descriptor(&self) -> ToolDescriptor {
        Tool::descriptor(self)
    }

    fn execute(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let output = Tool::execute(self, input, context)?;
        let rendered = self.render(&output);
        let output =
            serde_json::to_value(output).map_err(|e| ToolError::Serialization(e.to_string()))?;
        Ok(ToolResult { output, rendered })
    }
}

#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn ErasedTool>>,
}

impl ToolRegistry {
    pub fn with_builtins() -> Result<Self, ToolError> {
        let mut registry = Self::default();
        registry.register(super::glob::GlobTool)?;
        registry.register(super::grep::GrepTool)?;
        registry.register(super::list::ListTool)?;
        registry.register(super::read::ReadTool)?;
        Ok(registry)
    }

    pub fn register<T: Tool>(&mut self, tool: T) -> Result<(), ToolError> {
        let descriptor = tool.descriptor();
        if self.tools.contains_key(&descriptor.name) {
            return Err(ToolError::DuplicateTool(descriptor.name));
        }
        self.tools.insert(descriptor.name, Arc::new(tool));
        Ok(())
    }

    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools.values().map(|tool| tool.descriptor()).collect()
    }

    pub fn execute(
        &self,
        name: &str,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        self.tools
            .get(name)
            .ok_or_else(|| ToolError::UnknownTool(name.to_string()))?
            .execute(input, context)
    }
}
