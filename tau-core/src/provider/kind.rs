//! `Provider` enum — one variant per supported LLM provider.

use anyhow::Result;
use rig_core::client::CompletionClient;
use rig_core::completion::{CompletionError, CompletionRequest};
use rig_core::providers::{
    anthropic, azure, chatgpt, cohere, copilot, deepseek, gemini, groq, huggingface, hyperbolic,
    llamafile, minimax, mira, mistral, moonshot, ollama, openai, openrouter, perplexity, together,
    xai, xiaomimimo, zai,
};

use super::TauStream;

/// Concrete completion model holder, one variant per provider.
///
/// Created via [`Provider::new`] from a provider id + credentials. Stream via
/// [`Provider::stream`].
#[derive(Clone)]
pub enum Provider {
    Anthropic(anthropic::completion::CompletionModel),
    Azure(azure::CompletionModel),
    ChatGPT(chatgpt::ResponsesCompletionModel),
    Cohere(cohere::CompletionModel),
    Copilot(copilot::CompletionModel),
    DeepSeek(deepseek::CompletionModel),
    Gemini(gemini::CompletionModel),
    Groq(groq::CompletionModel),
    HuggingFace(huggingface::completion::CompletionModel),
    Hyperbolic(hyperbolic::CompletionModel),
    Llamafile(llamafile::CompletionModel),
    MiniMax(openai::completion::GenericCompletionModel<minimax::MiniMaxExt>),
    Mira(mira::CompletionModel),
    Mistral(mistral::CompletionModel),
    Moonshot(moonshot::CompletionModel),
    Ollama(ollama::CompletionModel),
    OpenAI(openai::responses_api::ResponsesCompletionModel),
    OpenRouter(openrouter::CompletionModel),
    Perplexity(perplexity::CompletionModel),
    Together(together::CompletionModel),
    XAI(xai::CompletionModel),
    XiaomiMimo(openai::completion::GenericCompletionModel<xiaomimimo::XiaomiMimoExt>),
    ZAI(openai::completion::GenericCompletionModel<zai::ZAiExt>),
    #[cfg(test)]
    Mock(rig_core::test_utils::MockCompletionModel),
}

/// Build a provider client (with optional base URL override), then create a
/// completion model from the model id.
macro_rules! make {
    ($module:ident, $variant:ident, $model_id:expr, $api_key:expr, $api_base:expr) => {{
        let api_key = $api_key
            .ok_or_else(|| anyhow::anyhow!("no API key configured for {}", stringify!($module)))?;
        let mut builder = $module::Client::builder().api_key(api_key.to_string());
        if let Some(url) = $api_base {
            builder = builder.base_url(url);
        }
        let client = builder
            .build()
            .map_err(|e| anyhow::anyhow!("{} client build: {e}", stringify!($module)))?;
        Ok(Self::$variant(client.completion_model($model_id)))
    }};
}

macro_rules! make_without_key {
    ($module:ident, $variant:ident, $model_id:expr, $api_base:expr) => {{
        let mut builder = $module::Client::builder().api_key(rig_core::client::Nothing);
        if let Some(url) = $api_base {
            builder = builder.base_url(url);
        }
        let client = builder
            .build()
            .map_err(|e| anyhow::anyhow!("{} client build: {e}", stringify!($module)))?;
        Ok(Self::$variant(client.completion_model($model_id)))
    }};
}

macro_rules! make_azure {
    ($model_id:expr, $api_key:expr, $api_base:expr) => {{
        let api_key = $api_key.ok_or_else(|| anyhow::anyhow!("no API key configured for azure"))?;
        let endpoint =
            $api_base.ok_or_else(|| anyhow::anyhow!("azure requires ProviderConfig.api_base"))?;
        let client = azure::Client::builder()
            .api_key(api_key.to_string())
            .azure_endpoint(endpoint.to_string())
            .build()
            .map_err(|e| anyhow::anyhow!("azure client build: {e}"))?;
        Ok(Self::Azure(client.completion_model($model_id)))
    }};
}

impl Provider {
    /// Construct the provider-specific completion model.
    ///
    /// `provider_id` is the lowercase id (e.g. `"openai"`), `model_id` is the
    /// provider-specific model name (e.g. `"gpt-4o"`), `api_key` is the
    /// resolved credential (when required), and `api_base` optionally overrides
    /// the base URL.
    pub fn new(
        provider_id: &str,
        model_id: impl Into<String>,
        api_key: Option<&str>,
        api_base: Option<&str>,
    ) -> Result<Self> {
        let model_id = model_id.into();
        match provider_id {
            "anthropic" => make!(anthropic, Anthropic, model_id, api_key, api_base),
            "azure" => make_azure!(model_id, api_key, api_base),
            "chatgpt" => make!(chatgpt, ChatGPT, model_id, api_key, api_base),
            "cohere" => make!(cohere, Cohere, model_id, api_key, api_base),
            "copilot" => make!(copilot, Copilot, model_id, api_key, api_base),
            "deepseek" => make!(deepseek, DeepSeek, model_id, api_key, api_base),
            "gemini" | "google" => make!(gemini, Gemini, model_id, api_key, api_base),
            "groq" => make!(groq, Groq, model_id, api_key, api_base),
            "huggingface" => make!(huggingface, HuggingFace, model_id, api_key, api_base),
            "hyperbolic" => make!(hyperbolic, Hyperbolic, model_id, api_key, api_base),
            "llamafile" => make_without_key!(llamafile, Llamafile, model_id, api_base),
            "minimax" => make!(minimax, MiniMax, model_id, api_key, api_base),
            "mira" => make!(mira, Mira, model_id, api_key, api_base),
            "mistral" => make!(mistral, Mistral, model_id, api_key, api_base),
            "moonshot" => make!(moonshot, Moonshot, model_id, api_key, api_base),
            "ollama" => make!(ollama, Ollama, model_id, api_key, api_base),
            "openai" => make!(openai, OpenAI, model_id, api_key, api_base),
            "openrouter" => make!(openrouter, OpenRouter, model_id, api_key, api_base),
            "perplexity" => make!(perplexity, Perplexity, model_id, api_key, api_base),
            "together" => make!(together, Together, model_id, api_key, api_base),
            "xai" => make!(xai, XAI, model_id, api_key, api_base),
            "xiaomimimo" => make!(xiaomimimo, XiaomiMimo, model_id, api_key, api_base),
            "zai" => make!(zai, ZAI, model_id, api_key, api_base),
            other => anyhow::bail!("unknown provider: {other}"),
        }
    }

    /// Stream a completion request, returning a provider-agnostic [`TauStream`].
    pub async fn stream(&self, request: CompletionRequest) -> Result<TauStream, CompletionError> {
        match self {
            Self::Anthropic(m) => super::ops::stream_with_model(m, request).await,
            Self::Azure(m) => super::ops::stream_with_model(m, request).await,
            Self::ChatGPT(m) => super::ops::stream_with_model(m, request).await,
            Self::Cohere(m) => super::ops::stream_with_model(m, request).await,
            Self::Copilot(m) => super::ops::stream_with_model(m, request).await,
            Self::DeepSeek(m) => super::ops::stream_with_model(m, request).await,
            Self::Gemini(m) => super::ops::stream_with_model(m, request).await,
            Self::Groq(m) => super::ops::stream_with_model(m, request).await,
            Self::HuggingFace(m) => super::ops::stream_with_model(m, request).await,
            Self::Hyperbolic(m) => super::ops::stream_with_model(m, request).await,
            Self::Llamafile(m) => super::ops::stream_with_model(m, request).await,
            Self::MiniMax(m) => super::ops::stream_with_model(m, request).await,
            Self::Mira(m) => super::ops::stream_with_model(m, request).await,
            Self::Mistral(m) => super::ops::stream_with_model(m, request).await,
            Self::Moonshot(m) => super::ops::stream_with_model(m, request).await,
            Self::Ollama(m) => super::ops::stream_with_model(m, request).await,
            Self::OpenAI(m) => super::ops::stream_with_model(m, request).await,
            Self::OpenRouter(m) => super::ops::stream_with_model(m, request).await,
            Self::Perplexity(m) => super::ops::stream_with_model(m, request).await,
            Self::Together(m) => super::ops::stream_with_model(m, request).await,
            Self::XAI(m) => super::ops::stream_with_model(m, request).await,
            Self::XiaomiMimo(m) => super::ops::stream_with_model(m, request).await,
            Self::ZAI(m) => super::ops::stream_with_model(m, request).await,
            #[cfg(test)]
            Self::Mock(m) => super::ops::stream_with_model(m, request).await,
        }
    }
}
