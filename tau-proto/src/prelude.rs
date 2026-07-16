//! Convenience re-exports — `use tau_proto::prelude::*` pulls in the everyday
//! protocol surface.

pub use crate::constants::*;
pub use crate::envelope::{Id, JsonRpc, Notification, Request, Response, RpcError};
pub use crate::health::{HealthResult, METHOD_HEALTH};
pub use crate::ping::METHOD_PING;
