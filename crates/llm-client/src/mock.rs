use async_trait::async_trait;
use std::sync::Mutex;

use crate::provider::*;

// ---------------------------------------------------------------------------
// MockProvider — returns canned responses for testing without API keys
// ---------------------------------------------------------------------------

pub struct MockProvider {
    name: String,
    model: String,
    /// Prompt substring → canned response. First match wins.
    responses: Mutex<Vec<(String, String)>>,
    /// Default response if no match is found
    default_response: String,
}

impl MockProvider {
    pub fn new(name: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            model: model.into(),
            responses: Mutex::new(Vec::new()),
            default_response: "No mock response configured for this prompt.".into(),
        }
    }

    /// Register a canned response: if the prompt contains `substring`, return `response`.
    pub fn on(self, substring: impl Into<String>, response: impl Into<String>) -> Self {
        self.responses
            .lock()
            .unwrap()
            .push((substring.into(), response.into()));
        self
    }

    pub fn with_default(mut self, response: impl Into<String>) -> Self {
        self.default_response = response.into();
        self
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    fn provider_name(&self) -> &str {
        &self.name
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let responses = self.responses.lock().unwrap();
        let content = responses
            .iter()
            .find(|(substring, _)| request.prompt.contains(substring.as_str()))
            .map(|(_, response)| response.clone())
            .unwrap_or_else(|| self.default_response.clone());

        Ok(CompletionResponse {
            content,
            usage: Some(TokenUsage {
                input_tokens: request.prompt.len() as u32,
                output_tokens: 50,
            }),
            latency_ms: 1,
        })
    }
}
