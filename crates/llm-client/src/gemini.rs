use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Instant;

use crate::provider::*;

const GEMINI_API_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";

// ---------------------------------------------------------------------------
// Gemini provider
// ---------------------------------------------------------------------------

pub struct GeminiProvider {
    client: Client,
    api_key: String,
    model: String,
}

impl GeminiProvider {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }

    /// Create from the GEMINI_API_KEY environment variable
    pub fn from_env(model: impl Into<String>) -> Result<Self, LlmError> {
        let api_key = std::env::var("GEMINI_API_KEY").map_err(|_| LlmError::MissingApiKey {
            provider: "google".into(),
        })?;
        Ok(Self::new(api_key, model))
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    fn provider_name(&self) -> &str {
        "google"
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let url = format!(
            "{}/{}:generateContent?key={}",
            GEMINI_API_URL, self.model, self.api_key
        );

        let contents = vec![GeminiContent {
            role: "user".into(),
            parts: vec![GeminiPart {
                text: request.prompt,
            }],
        }];

        let system_instruction = request.system.map(|s| GeminiContent {
            role: "user".into(),
            parts: vec![GeminiPart { text: s }],
        });

        let body = GeminiRequest {
            contents,
            system_instruction,
            generation_config: Some(GeminiGenerationConfig {
                max_output_tokens: Some(request.max_tokens),
                temperature: Some(request.temperature),
            }),
        };

        let start = Instant::now();

        let response = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let latency_ms = start.elapsed().as_millis() as u64;
        let status = response.status().as_u16();

        if status != 200 {
            let error_text = response.text().await.unwrap_or_default();
            return Err(LlmError::Api {
                provider: "google".into(),
                status,
                message: error_text,
            });
        }

        let api_response: GeminiResponse =
            response.json().await.map_err(|e| LlmError::Parse {
                provider: "google".into(),
                message: e.to_string(),
            })?;

        let content = api_response
            .candidates
            .into_iter()
            .flat_map(|c| c.content.parts)
            .map(|p| p.text)
            .collect::<Vec<_>>()
            .join("");

        let usage = api_response.usage_metadata.map(|u| TokenUsage {
            input_tokens: u.prompt_token_count,
            output_tokens: u.candidates_token_count,
        });

        Ok(CompletionResponse {
            content,
            usage,
            latency_ms,
        })
    }
}

// ---------------------------------------------------------------------------
// Gemini API types (private)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Serialize)]
struct GeminiContent {
    role: String,
    parts: Vec<GeminiPart>,
}

#[derive(Serialize, Deserialize)]
struct GeminiPart {
    text: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: GeminiResponseContent,
}

#[derive(Deserialize)]
struct GeminiResponseContent {
    parts: Vec<GeminiPart>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsageMetadata {
    prompt_token_count: u32,
    candidates_token_count: u32,
}
