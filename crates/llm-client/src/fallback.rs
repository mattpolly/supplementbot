use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::timeout;

use crate::provider::{CompletionRequest, CompletionResponse, LlmError, LlmProvider};

// ---------------------------------------------------------------------------
// FallbackProvider — wraps a primary + fallback with per-call timeout.
//
// If the primary times out or returns an error, the request is retried
// against the fallback. Both providers are tried with the same timeout.
//
// Usage:
//   let provider = FallbackProvider::new(anthropic, gemini, Duration::from_secs(20));
// ---------------------------------------------------------------------------

pub struct FallbackProvider {
    primary: Arc<dyn LlmProvider>,
    fallback: Arc<dyn LlmProvider>,
    timeout: Duration,
}

impl FallbackProvider {
    pub fn new(
        primary: Arc<dyn LlmProvider>,
        fallback: Arc<dyn LlmProvider>,
        call_timeout: Duration,
    ) -> Self {
        Self {
            primary,
            fallback,
            timeout: call_timeout,
        }
    }
}

#[async_trait]
impl LlmProvider for FallbackProvider {
    fn provider_name(&self) -> &str {
        self.primary.provider_name()
    }

    fn model_name(&self) -> &str {
        self.primary.model_name()
    }

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        // Try primary with timeout
        let primary_result = timeout(self.timeout, self.primary.complete(request.clone())).await;

        match primary_result {
            Ok(Ok(response)) => return Ok(response),
            Ok(Err(e)) => {
                eprintln!(
                    "[fallback] primary ({}/{}) error: {e} — trying fallback",
                    self.primary.provider_name(),
                    self.primary.model_name()
                );
            }
            Err(_) => {
                eprintln!(
                    "[fallback] primary ({}/{}) timed out after {}s — trying fallback",
                    self.primary.provider_name(),
                    self.primary.model_name(),
                    self.timeout.as_secs()
                );
            }
        }

        // Try fallback with timeout
        let fallback_result = timeout(self.timeout, self.fallback.complete(request)).await;

        match fallback_result {
            Ok(result) => result,
            Err(_) => Err(LlmError::Api {
                provider: self.fallback.provider_name().to_string(),
                status: 0,
                message: format!(
                    "fallback ({}/{}) also timed out after {}s",
                    self.fallback.provider_name(),
                    self.fallback.model_name(),
                    self.timeout.as_secs()
                ),
            }),
        }
    }
}
