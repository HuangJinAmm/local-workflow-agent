pub mod anthropic;
pub use anthropic::AnthropicProvider;

pub(crate) mod message_normalization;
pub(crate) mod request_options;

pub mod openai;
pub use openai::OpenAiProvider;

pub mod openai_compat;
pub use openai_compat::OpenAiCompatProvider;

pub mod openai_compat_providers;
pub use openai_compat_providers::{
    baseten, cerebras, deepinfra, deepseek, fireworks, friendli, groq, huggingface, llama_cpp,
    lm_studio, mistral, moonshot, nebius, novita, nvidia, ollama, opencode_zen, openrouter,
    ovhcloud, perplexity, qwen, sambanova, scaleway, siliconflow, stepfun, together_ai, upstage,
    venice, vultr_ai, xai, zai, zhipu,
};
