//! Internal typed tools used by the future agent runner.

mod error;
mod policy;
mod registry;
mod types;

pub mod glob;
pub mod grep;
pub mod hashline;
pub mod list;
pub mod read;

pub use error::ToolError;
pub use policy::{AccessPolicy, ResolvedPath};
pub use registry::{Tool, ToolDescriptor, ToolRegistry, ToolResult};
pub use types::{
    BinaryRead, DirectoryEntry, DirectoryRead, EntryKind, FileRead, GlobOutput, GrepMatch,
    GrepOutput, ListOutput, ReadOutput, ToolContext, ToolLimits,
};

#[cfg(test)]
mod tests;
