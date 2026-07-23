//! tau — wire protocol.
//!
//! Pure serde types describing the tau JSON-RPC 2.0 protocol carried over the
//! daemon's WebSocket. No network code, no tokio, no agent logic. Shared by
//! `tau-server` and `tau-client` (and any future client emitter) so both sides
//! of the socket speak one typed contract.
//!
//! Frame convention: text WebSocket frames carry JSON-RPC 2.0; binary frames
//! are reserved for multiplexed opaque channels (see [`binary`]).

pub mod auth;
pub mod completion;
pub mod constants;
pub mod envelope;
pub mod git;
pub mod health;
pub mod ping;
pub mod policy;
pub mod prelude;
pub mod projects;
pub mod session;
pub mod turn;

/// Reserved binary-frame channel convention.
///
/// Text WebSocket frames always carry JSON-RPC 2.0. Binary frames carry a
/// one-byte channel id followed by that channel's opaque payload:
/// `[u8 channel][payload bytes]`. No channel is implemented in M0; the daemon
/// logs and ignores binary frames. Channels land alongside the tools that need
/// them (e.g. PTY for the `bash` tool).
pub mod binary {
    /// Pseudo-terminal byte stream (for the `bash` tool).
    pub const CH_PTY: u8 = 0x01;
    /// File / attachment blob transfer.
    pub const CH_ATTACHMENT: u8 = 0x02;
}

// keep JsonRpc referenced so future wire additions land cleanly
const _: envelope::JsonRpc = envelope::JsonRpc::V2;
