use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// LlmProvider — the trait every provider implements
// ---------------------------------------------------------------------------

#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Human-readable provider name (e.g. "anthropic", "google", "xai")
    fn provider_name(&self) -> &str;

    /// Model identifier (e.g. "claude-sonnet-4-20250514", "gemini-2.0-flash")
    fn model_name(&self) -> &str;

    /// Send a prompt and get a response. This is the only method providers need to implement.
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError>;
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    /// The system prompt — sets the LLM's role and constraints
    pub system: Option<String>,
    /// The user prompt — the actual question
    pub prompt: String,
    /// Max tokens to generate
    pub max_tokens: u32,
    /// Temperature (0.0 = deterministic, 1.0 = creative)
    pub temperature: f32,
}

impl CompletionRequest {
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            system: None,
            prompt: prompt.into(),
            max_tokens: 4096,
            temperature: 0.3,
        }
    }

    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = temperature;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResponse {
    /// The generated text
    pub content: String,
    /// Token usage stats
    pub usage: Option<TokenUsage>,
    /// How long the request took in milliseconds
    pub latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API error from {provider}: {message} (status: {status})")]
    Api {
        provider: String,
        status: u16,
        message: String,
    },

    #[error("Failed to parse response from {provider}: {message}")]
    Parse { provider: String, message: String },

    #[error("Missing API key for {provider}")]
    MissingApiKey { provider: String },
}
