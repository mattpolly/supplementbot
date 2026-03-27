use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Instant;

use crate::provider::*;

const XAI_API_URL: &str = "https://api.x.ai/v1/chat/completions";

// ---------------------------------------------------------------------------
// xAI (Grok) provider — OpenAI-compatible chat completions API
// ---------------------------------------------------------------------------

pub struct XaiProvider {
    client: Client,
    api_key: String,
    model: String,
}

impl XaiProvider {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }

    /// Create from the XAI_API_KEY environment variable
    pub fn from_env(model: impl Into<String>) -> Result<Self, LlmError> {
        let api_key = std::env::var("XAI_API_KEY").map_err(|_| LlmError::MissingApiKey {
            provider: "xai".into(),
        })?;
        Ok(Self::new(api_key, model))
    }
}

#[async_trait]
impl LlmProvider for XaiProvider {
    fn provider_name(&self) -> &str {
        "xai"
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let mut messages = Vec::new();

        if let Some(system) = request.system {
            messages.push(ChatMessage {
                role: "system".into(),
                content: system,
            });
        }

        messages.push(ChatMessage {
            role: "user".into(),
            content: request.prompt,
        });

        let body = ChatCompletionRequest {
            model: self.model.clone(),
            messages,
            max_tokens: Some(request.max_tokens),
            temperature: Some(request.temperature),
        };

        let start = Instant::now();

        let response = self
            .client
            .post(XAI_API_URL)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let latency_ms = start.elapsed().as_millis() as u64;
        let status = response.status().as_u16();

        if status != 200 {
            let error_text = response.text().await.unwrap_or_default();
            return Err(LlmError::Api {
                provider: "xai".into(),
                status,
                message: error_text,
            });
        }

        let api_response: ChatCompletionResponse =
            response.json().await.map_err(|e| LlmError::Parse {
                provider: "xai".into(),
                message: e.to_string(),
            })?;

        let content = api_response
            .choices
            .into_iter()
            .filter_map(|c| Some(c.message.content))
            .collect::<Vec<_>>()
            .join("");

        let usage = api_response.usage.map(|u| TokenUsage {
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
        });

        Ok(CompletionResponse {
            content,
            usage,
            latency_ms,
        })
    }
}

// ---------------------------------------------------------------------------
// OpenAI-compatible API types (private)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
    usage: Option<ChatUsage>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    content: String,
}

#[derive(Deserialize)]
struct ChatUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
}
