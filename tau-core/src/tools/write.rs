use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::error::{ToolError, io};
use super::registry::{Tool, ToolDescriptor};
use super::snapshot::SnapshotStore;
use super::types::ToolContext;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteInput {
    pub path: PathBuf,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WriteOutput {
    pub path: PathBuf,
    pub existed: bool,
    pub bytes: usize,
    pub snapshot_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct WriteTool;

impl Tool for WriteTool {
    type Input = WriteInput;
    type Output = WriteOutput;

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "write".into(),
            description: "Atomically create or overwrite a file inside an approved path root."
                .into(),
        }
    }

    fn execute(&self, input: WriteInput, context: &ToolContext) -> Result<WriteOutput, ToolError> {
        let resolved = context.policy.resolve(&context.cwd, &input.path, "write")?;
        let path = resolved.path;
        let output_path = path.clone();
        context.mutation.with_path_lock(&path, || {
            if path.exists()
                && std::fs::metadata(&path)
                    .map_err(|e| io("stat", &path, e))?
                    .is_dir()
            {
                return Err(ToolError::NotFile(path.clone()));
            }
            let snapshot =
                SnapshotStore::for_cwd(&context.cwd).capture_paths(std::slice::from_ref(&path))?;
            let existed = path.exists();
            if existed {
                let expected =
                    std::fs::read(&path).map_err(|e| io("read before write", &path, e))?;
                context.mutation.replace_if_unchanged(
                    &path,
                    &expected,
                    input.content.as_bytes(),
                )?;
            } else {
                context
                    .mutation
                    .atomic_replace(&path, input.content.as_bytes())?;
            }
            Ok(WriteOutput {
                path: output_path,
                existed,
                bytes: input.content.len(),
                snapshot_id: snapshot.id,
            })
        })
    }

    fn render(&self, output: &WriteOutput) -> String {
        format!(
            "Wrote {} ({} bytes, snapshot {}).",
            output.path.display(),
            output.bytes,
            output.snapshot_id
        )
    }
}
