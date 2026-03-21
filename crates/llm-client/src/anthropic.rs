use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Instant;

use crate::provider::*;

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

// ---------------------------------------------------------------------------
// Anthropic provider
// ---------------------------------------------------------------------------

pub struct AnthropicProvider {
    client: Client,
    api_key: String,
    model: String,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }

    /// Create from the ANTHROPIC_API_KEY environment variable
    pub fn from_env(model: impl Into<String>) -> Result<Self, LlmError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| LlmError::MissingApiKey {
            provider: "anthropic".into(),
        })?;
        Ok(Self::new(api_key, model))
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn provider_name(&self) -> &str {
        "anthropic"
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let body = AnthropicRequest {
            model: self.model.clone(),
            max_tokens: request.max_tokens,
            system: request.system,
            messages: vec![AnthropicMessage {
                role: "user".into(),
                content: request.prompt,
            }],
            temperature: Some(request.temperature),
        };

        let start = Instant::now();

        let response = self
            .client
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let latency_ms = start.elapsed().as_millis() as u64;
        let status = response.status().as_u16();

        if status != 200 {
            let error_text = response.text().await.unwrap_or_default();
            return Err(LlmError::Api {
                provider: "anthropic".into(),
                status,
                message: error_text,
            });
        }

        let api_response: AnthropicResponse =
            response.json().await.map_err(|e| LlmError::Parse {
                provider: "anthropic".into(),
                message: e.to_string(),
            })?;

        let content = api_response
            .content
            .into_iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text),
            })
            .collect::<Vec<_>>()
            .join("");

        Ok(CompletionResponse {
            content,
            usage: api_response.usage.map(|u| TokenUsage {
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
            }),
            latency_ms,
        })
    }
}

// ---------------------------------------------------------------------------
// Anthropic API types (private)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<ContentBlock>,
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
}

#[derive(Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}
