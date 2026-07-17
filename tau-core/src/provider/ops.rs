//! Generic streaming helpers shared across all provider variants.

use std::future::ready;

use futures::stream::StreamExt;
use rig_core::OneOrMany;
use rig_core::completion::{
    CompletionError, CompletionModel, CompletionRequest, GetTokenUsage, Message,
};
use rig_core::streaming::{StreamedAssistantContent, StreamingCompletionResponse};

use super::{TauDelta, TauStream};

/// Stream a completion request through a concrete model, normalising the
/// provider-specific [`StreamingCompletionResponse`] into [`TauStream`].
pub async fn stream_with_model<M: CompletionModel>(
    model: &M,
    request: CompletionRequest,
) -> Result<TauStream, CompletionError>
where
    M::StreamingResponse: 'static,
{
    let stream: StreamingCompletionResponse<M::StreamingResponse> = model.stream(request).await?;
    let mapped = stream.filter_map(|item| ready(extract_delta::<M::StreamingResponse>(item)));
    Ok(mapped.boxed())
}

/// Convert a [`StreamedAssistantContent`] item into a [`TauDelta`], dropping
/// non-text/non-final items (tool calls, reasoning, unknown).
fn extract_delta<R: GetTokenUsage>(
    item: Result<StreamedAssistantContent<R>, CompletionError>,
) -> Option<Result<TauDelta, CompletionError>> {
    match item {
        Ok(StreamedAssistantContent::Text(t)) => Some(Ok(TauDelta::Text(t.text))),
        Ok(StreamedAssistantContent::ToolCall { tool_call, .. }) => {
            Some(Ok(TauDelta::ToolCall(tool_call)))
        }
        Ok(StreamedAssistantContent::Final(r)) => Some(Ok(TauDelta::Usage(r.token_usage()))),
        Ok(_) => None,
        Err(e) => Some(Err(e)),
    }
}

/// Build a minimal [`CompletionRequest`] from a single prompt message.
///
/// The prompt becomes the sole element of `chat_history`. For multi-turn
/// conversations the caller should construct the request directly with prior
/// messages prepended.
pub fn completion_request(prompt: impl Into<Message>) -> CompletionRequest {
    CompletionRequest {
        model: None,
        preamble: None,
        chat_history: OneOrMany::one(prompt.into()),
        documents: vec![],
        tools: vec![],
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        additional_params: None,
        output_schema: None,
    }
}
