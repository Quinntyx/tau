//! Internal typed tools used by the future agent runner.

mod error;
mod mutation;
mod policy;
mod registry;
mod snapshot;
pub mod transaction;
mod types;

pub mod bash;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod hashline;
pub mod list;
pub mod read;
pub mod write;

pub use bash::{BashInput, BashOutput, BashTool, CommandClass, classify_command};
pub use edit::{EditInput, EditOperation, EditOutput, EditTool};
pub use error::ToolError;
pub use glob::{GlobInput, GlobTool};
pub use grep::{GrepInput, GrepTool};
pub use list::{ListInput, ListTool};
pub use mutation::MutationCoordinator;
pub use policy::{AccessPolicy, ResolvedPath};
pub use read::{ReadInput, ReadTool};
pub use registry::{Tool, ToolDescriptor, ToolRegistry, ToolResult, schema_for};
pub use snapshot::{SnapshotCapture, SnapshotStore};
pub use transaction::{DiffFile, DiffHunk, FileDecision, SnapshotTransaction, TransactionError};
pub use types::{
    BinaryRead, DirectoryEntry, DirectoryRead, EntryKind, FileRead, GlobOutput, GrepMatch,
    GrepOutput, ListOutput, ReadOutput, ToolContext, ToolLimits,
};
pub use write::{WriteInput, WriteOutput, WriteTool};

#[cfg(test)]
mod tests;
