//! Convenience re-exports — `use tau_proto::prelude::*` pulls in the everyday
//! protocol surface.

pub use crate::completion::{
    CompletionDelta, CompletionStreamParams, CompletionStreamResult, METHOD_COMPLETION_DELTA,
    METHOD_COMPLETION_STREAM, UsageSummary,
};
pub use crate::constants::*;
pub use crate::envelope::{Id, JsonRpc, Notification, Request, Response, RpcError};
pub use crate::git::*;
pub use crate::health::{HealthResult, METHOD_HEALTH};
pub use crate::ping::METHOD_PING;
pub use crate::policy::*;
pub use crate::projects::*;
pub use crate::session::*;
pub use crate::turn::*;
