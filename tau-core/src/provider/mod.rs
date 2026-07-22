//! Provider dispatch — maps a provider id to a concrete rig completion model.
//!
//! `CompletionModel` is not object-safe (associated types), so tau uses a
//! concrete [`Provider`] enum with one variant per supported provider. Each
//! match arm delegates to [`stream_with_model`], a generic helper that
//! normalises the provider-specific streaming response into [`TauStream`].

mod kind;
mod ops;
mod test;

pub use kind::Provider;
pub use ops::{completion_request, stream_with_model};
pub use test::{ENABLE_ENV as TEST_PROVIDER_ENABLE_ENV, FIXTURE_ENV as TEST_PROVIDER_FIXTURE_ENV};

use std::pin::Pin;

use futures::Stream;
use rig_core::completion::{CompletionError, Usage, message::ToolCall};

/// Normalised streaming delta — text chunks and final usage.
#[derive(Debug, Clone)]
pub enum TauDelta {
    /// Text chunk from the assistant.
    Text(String),
    /// A complete function call requested by the model.
    ToolCall(ToolCall),
    /// Token usage from the final response.
    Usage(Usage),
}

/// Boxed, provider-agnostic streaming completion stream.
pub type TauStream = Pin<Box<dyn Stream<Item = Result<TauDelta, CompletionError>> + Send>>;

#[cfg(test)]
mod tests;
