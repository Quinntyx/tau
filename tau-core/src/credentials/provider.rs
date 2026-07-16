//! Known LLM providers, keyed by strum-derivable identifiers.
//!
//! The [`Provider`] enum replaces the old `(&str, &str)` constant arrays.
//! Strum gives us `Display` (to string), `FromStr` (from string), `AsRefStr`,
//! and `EnumIter` (iterate all variants) for free — no hand-maintained lookup
//! tables.

use std::str::FromStr;

use strum::{AsRefStr, Display, EnumIter, EnumString};

/// Every LLM provider tau recognises. Variant names are lowercased by strum to
/// form the canonical provider id (e.g. `OpenAI` → `"openai"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, EnumString, AsRefStr, Display)]
#[strum(serialize_all = "lowercase")]
pub enum Provider {
    Anthropic,
    OpenAI,
    Gemini,
    Google,
    Groq,
    Mistral,
    DeepSeek,
    XAI,
    OpenRouter,
}

impl Provider {
    /// The environment variable that conventionally holds this provider's
    /// API key.
    pub fn api_key_env(self) -> &'static str {
        match self {
            Self::Anthropic => "ANTHROPIC_API_KEY",
            Self::OpenAI => "OPENAI_API_KEY",
            Self::Gemini | Self::Google => "GEMINI_API_KEY",
            Self::Groq => "GROQ_API_KEY",
            Self::Mistral => "MISTRAL_API_KEY",
            Self::DeepSeek => "DEEPSEEK_API_KEY",
            Self::XAI => "XAI_API_KEY",
            Self::OpenRouter => "OPENROUTER_API_KEY",
        }
    }

    /// Resolve the env-var name for any provider id — known or unknown.
    /// Known providers use [`Provider::api_key_env`]; unknown ids fall back to
    /// the `{UPPER}_API_KEY` convention.
    pub fn env_var_name(provider: &str) -> String {
        match Provider::from_str(provider) {
            Ok(p) => p.api_key_env().to_string(),
            Err(_) => {
                let upper = provider.to_ascii_uppercase().replace('-', "_");
                format!("{upper}_API_KEY")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;

    #[test]
    fn round_trip() {
        for p in Provider::iter() {
            let s = p.to_string();
            assert_eq!(Provider::from_str(&s).unwrap(), p);
        }
    }

    #[test]
    fn api_key_env_known() {
        assert_eq!(Provider::Anthropic.api_key_env(), "ANTHROPIC_API_KEY");
        assert_eq!(Provider::OpenAI.api_key_env(), "OPENAI_API_KEY");
    }

    #[test]
    fn env_var_name_fallback() {
        assert_eq!(Provider::env_var_name("my-provider"), "MY_PROVIDER_API_KEY");
        assert_eq!(Provider::env_var_name("anthropic"), "ANTHROPIC_API_KEY");
    }
}
