//! `Provider` enum — one variant per supported LLM provider.

use anyhow::Result;
use rig_core::client::CompletionClient;
use rig_core::completion::{CompletionError, CompletionRequest};
use rig_core::providers::{anthropic, deepseek, gemini, groq, mistral, openai, openrouter, xai};

use super::TauStream;

/// Concrete completion model holder, one variant per provider.
///
/// Created via [`Provider::new`] from a provider id + credentials. Stream via
/// [`Provider::stream`].
#[derive(Clone)]
pub enum Provider {
    Anthropic(anthropic::completion::CompletionModel),
    OpenAI(openai::responses_api::ResponsesCompletionModel),
    Gemini(gemini::CompletionModel),
    Groq(groq::CompletionModel),
    Mistral(mistral::CompletionModel),
    DeepSeek(deepseek::CompletionModel),
    Xai(xai::CompletionModel),
    OpenRouter(openrouter::CompletionModel),
    #[cfg(test)]
    Mock(rig_core::test_utils::MockCompletionModel),
}

/// Build a provider client (with optional base URL override), then create a
/// completion model from the model id.
macro_rules! make {
    ($module:ident, $variant:ident, $model_id:expr, $api_key:expr, $api_base:expr) => {{
        let mut builder = $module::Client::builder().api_key($api_key.to_string());
        if let Some(url) = $api_base {
            builder = builder.base_url(url);
        }
        let client = builder
            .build()
            .map_err(|e| anyhow::anyhow!("{} client build: {e}", stringify!($module)))?;
        Ok(Self::$variant(client.completion_model($model_id)))
    }};
}

impl Provider {
    /// Construct the provider-specific completion model.
    ///
    /// `provider_id` is the lowercase id (e.g. `"openai"`), `model_id` is the
    /// provider-specific model name (e.g. `"gpt-4o"`), `api_key` is the
    /// resolved credential, and `api_base` optionally overrides the base URL.
    pub fn new(
        provider_id: &str,
        model_id: impl Into<String>,
        api_key: &str,
        api_base: Option<&str>,
    ) -> Result<Self> {
        let model_id = model_id.into();
        match provider_id {
            "anthropic" => make!(anthropic, Anthropic, model_id, api_key, api_base),
            "openai" => make!(openai, OpenAI, model_id, api_key, api_base),
            "gemini" | "google" => make!(gemini, Gemini, model_id, api_key, api_base),
            "groq" => make!(groq, Groq, model_id, api_key, api_base),
            "mistral" => make!(mistral, Mistral, model_id, api_key, api_base),
            "deepseek" => make!(deepseek, DeepSeek, model_id, api_key, api_base),
            "xai" => make!(xai, Xai, model_id, api_key, api_base),
            "openrouter" => make!(openrouter, OpenRouter, model_id, api_key, api_base),
            other => anyhow::bail!("unknown provider: {other}"),
        }
    }

    /// Stream a completion request, returning a provider-agnostic [`TauStream`].
    pub async fn stream(&self, request: CompletionRequest) -> Result<TauStream, CompletionError> {
        match self {
            Self::Anthropic(m) => super::ops::stream_with_model(m, request).await,
            Self::OpenAI(m) => super::ops::stream_with_model(m, request).await,
            Self::Gemini(m) => super::ops::stream_with_model(m, request).await,
            Self::Groq(m) => super::ops::stream_with_model(m, request).await,
            Self::Mistral(m) => super::ops::stream_with_model(m, request).await,
            Self::DeepSeek(m) => super::ops::stream_with_model(m, request).await,
            Self::Xai(m) => super::ops::stream_with_model(m, request).await,
            Self::OpenRouter(m) => super::ops::stream_with_model(m, request).await,
            #[cfg(test)]
            Self::Mock(m) => super::ops::stream_with_model(m, request).await,
        }
    }
}
