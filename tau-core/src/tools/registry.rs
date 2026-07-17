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

/// Provider-facing JSON schema.  Keeping this at the registry boundary means
/// typed tools remain independent of Rig while providers never receive the
/// old, unusable `{type: object}` placeholder.
pub fn schema_for(name: &str) -> serde_json::Value {
    let string = |required: &[&str]| {
        serde_json::json!({
            "type":"object", "properties": { "file_path":{"type":"string"}, "path":{"type":"string"}, "command":{"type":"string"}, "pattern":{"type":"string"}, "include":{"type":"string"}, "content":{"type":"string"}, "ref":{"type":"string"}, "start_ref":{"type":"string"}, "end_ref":{"type":"string"}, "workdir":{"type":"string"} }, "required":required, "additionalProperties":false
        })
    };
    match name {
        "read" => {
            serde_json::json!({"type":"object","properties":{"file_path":{"type":"string"},"offset":{"type":"integer","minimum":1},"limit":{"type":"integer","minimum":1}},"required":["file_path"],"additionalProperties":false})
        }
        "write" => {
            serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false})
        }
        "bash" => {
            serde_json::json!({"type":"object","properties":{"command":{"type":"string"},"workdir":{"type":"string"},"timeout":{"type":"integer","minimum":1}},"required":["command"],"additionalProperties":false})
        }
        "glob" => string(&["pattern"]),
        "grep" => string(&["pattern"]),
        "list" => {
            serde_json::json!({"type":"object","properties":{"path":{"type":"string"}},"additionalProperties":false})
        }
        "edit" => string(&["path"]),
        _ => serde_json::json!({"type":"object","additionalProperties":false}),
    }
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
        registry.register(super::bash::BashTool)?;
        registry.register(super::edit::EditTool)?;
        registry.register(super::glob::GlobTool)?;
        registry.register(super::grep::GrepTool)?;
        registry.register(super::list::ListTool)?;
        registry.register(super::read::ReadTool)?;
        registry.register(super::write::WriteTool)?;
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

    pub fn schemas(&self) -> Vec<(ToolDescriptor, serde_json::Value)> {
        self.descriptors()
            .into_iter()
            .map(|d| {
                let schema = schema_for(&d.name);
                (d, schema)
            })
            .collect()
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
