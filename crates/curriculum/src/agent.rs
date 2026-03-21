use event_log::events::{CurriculumStage, PipelineEvent, TokenUsage as EventTokenUsage};
use event_log::sink::EventSink;
use llm_client::provider::{CompletionRequest, LlmProvider};
use uuid::Uuid;

use crate::questions::{self, CurriculumQuestion, Stage};

// ---------------------------------------------------------------------------
// CurriculumResponse — what comes back from running one question
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CurriculumResponse {
    pub question: CurriculumQuestion,
    pub raw_response: String,
    pub latency_ms: u64,
}

// ---------------------------------------------------------------------------
// CurriculumAgent — runs questions through an LLM provider with observability
// ---------------------------------------------------------------------------

pub struct CurriculumAgent<'a> {
    provider: &'a dyn LlmProvider,
    sink: &'a dyn EventSink,
}

impl<'a> CurriculumAgent<'a> {
    pub fn new(provider: &'a dyn LlmProvider, sink: &'a dyn EventSink) -> Self {
        Self { provider, sink }
    }

    /// Run all Stage 1 questions for a nutraceutical. Returns responses in order.
    pub async fn run_stage1(
        &self,
        nutraceutical: &str,
        correlation_id: Uuid,
    ) -> Vec<Result<CurriculumResponse, String>> {
        let questions = questions::stage1_questions(nutraceutical);
        let mut results = Vec::with_capacity(questions.len());

        for question in questions {
            let result = self.ask(&question, correlation_id).await;
            results.push(result);
        }

        results
    }

    /// Run all Stage 2 questions for a nutraceutical given its known systems.
    pub async fn run_stage2(
        &self,
        nutraceutical: &str,
        related_systems: &[&str],
        correlation_id: Uuid,
    ) -> Vec<Result<CurriculumResponse, String>> {
        let questions = questions::stage2_questions(nutraceutical, related_systems);
        let mut results = Vec::with_capacity(questions.len());

        for question in questions {
            let result = self.ask(&question, correlation_id).await;
            results.push(result);
        }

        results
    }

    /// Ask a single question — emits events before and after
    async fn ask(
        &self,
        question: &CurriculumQuestion,
        correlation_id: Uuid,
    ) -> Result<CurriculumResponse, String> {
        let stage = match question.stage {
            Stage::Foundational => CurriculumStage::Foundational,
            Stage::Relational => CurriculumStage::Relational,
        };

        // Emit: request is about to go out
        self.sink.emit(
            correlation_id,
            PipelineEvent::LlmRequest {
                provider: self.provider.provider_name().to_string(),
                model: self.provider.model_name().to_string(),
                prompt: question.prompt.clone(),
                nutraceutical: question.nutraceutical.clone(),
                stage: stage.clone(),
                question_type: format!("{:?}", question.question_type),
            },
        );

        let request = CompletionRequest::new(&question.prompt)
            .with_system(questions::system_prompt().to_string());

        match self.provider.complete(request).await {
            Ok(response) => {
                // Emit: response came back
                self.sink.emit(
                    correlation_id,
                    PipelineEvent::LlmResponse {
                        provider: self.provider.provider_name().to_string(),
                        model: self.provider.model_name().to_string(),
                        raw_response: response.content.clone(),
                        latency_ms: response.latency_ms,
                        tokens_used: response.usage.map(|u| EventTokenUsage {
                            input_tokens: u.input_tokens,
                            output_tokens: u.output_tokens,
                        }),
                    },
                );

                Ok(CurriculumResponse {
                    question: question.clone(),
                    raw_response: response.content,
                    latency_ms: response.latency_ms,
                })
            }
            Err(e) => {
                let error_msg = e.to_string();

                // Emit: error occurred
                self.sink.emit(
                    correlation_id,
                    PipelineEvent::LlmError {
                        provider: self.provider.provider_name().to_string(),
                        model: self.provider.model_name().to_string(),
                        error: error_msg.clone(),
                    },
                );

                Err(error_msg)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use event_log::events::PipelineEvent;
    use event_log::sink::MemorySink;
    use llm_client::mock::MockProvider;
    use uuid::Uuid;

    fn mock_provider() -> MockProvider {
        MockProvider::new("mock", "mock-v1")
            .on(
                "physiological systems",
                "Magnesium acts on: 1) Nervous system — NMDA receptor antagonism, \
                 GABA modulation. 2) Gastrointestinal — smooth muscle relaxation, \
                 motility regulation. 3) Musculoskeletal — muscle contraction/relaxation \
                 via calcium channel regulation. 4) Immune — modulates inflammatory cytokines.",
            )
            .on(
                "mechanisms of action",
                "Mechanisms of action for Magnesium: 1) NMDA receptor antagonism — \
                 blocks calcium influx at NMDA receptors. 2) GABA-A receptor positive \
                 allosteric modulation. 3) Calcium channel regulation — competes with \
                 calcium at voltage-gated channels. 4) Cytokine modulation — reduces \
                 NF-kB activation.",
            )
            .on(
                "therapeutic uses",
                "Therapeutic uses of Magnesium: 1) Muscle cramping and tension — via \
                 calcium channel regulation. 2) Sleep difficulty — via GABA modulation \
                 and NMDA antagonism. 3) Stress-related symptoms — via HPA axis modulation. \
                 4) Irregular bowel motility — via smooth muscle relaxation in the GI tract.",
            )
    }

    #[tokio::test]
    async fn test_stage1_runs_all_questions() {
        let provider = mock_provider();
        let sink = MemorySink::new();
        let agent = CurriculumAgent::new(&provider, &sink);
        let corr_id = Uuid::new_v4();

        let results = agent.run_stage1("Magnesium", corr_id).await;

        assert_eq!(results.len(), 3);
        for result in &results {
            assert!(result.is_ok());
        }

        // Verify we got meaningful responses
        let r0 = results[0].as_ref().unwrap();
        assert!(r0.raw_response.contains("Nervous system"));
        assert!(r0.raw_response.contains("Gastrointestinal"));
    }

    #[tokio::test]
    async fn test_event_emission() {
        let provider = mock_provider();
        let sink = MemorySink::new();
        let agent = CurriculumAgent::new(&provider, &sink);
        let corr_id = Uuid::new_v4();

        agent.run_stage1("Magnesium", corr_id).await;

        let events = sink.events_for(corr_id);

        // 3 questions × 2 events each (request + response) = 6 events
        assert_eq!(events.len(), 6);

        // Verify alternating request/response pattern
        for (i, event) in events.iter().enumerate() {
            match &event.event {
                PipelineEvent::LlmRequest { .. } => assert_eq!(i % 2, 0, "requests on even indices"),
                PipelineEvent::LlmResponse { .. } => {
                    assert_eq!(i % 2, 1, "responses on odd indices")
                }
                other => panic!("unexpected event type: {:?}", other),
            }
        }

        // Verify first request has correct metadata
        match &events[0].event {
            PipelineEvent::LlmRequest {
                provider,
                nutraceutical,
                question_type,
                ..
            } => {
                assert_eq!(provider, "mock");
                assert_eq!(nutraceutical, "Magnesium");
                assert_eq!(question_type, "Systems");
            }
            _ => panic!("expected LlmRequest"),
        }
    }

    #[tokio::test]
    async fn test_correlation_id_groups_events() {
        let provider = mock_provider();
        let sink = MemorySink::new();
        let agent = CurriculumAgent::new(&provider, &sink);

        let corr_mag = Uuid::new_v4();
        let corr_zinc = Uuid::new_v4();

        agent.run_stage1("Magnesium", corr_mag).await;
        agent.run_stage1("Zinc", corr_zinc).await;

        // Total events: 6 + 6 = 12
        assert_eq!(sink.len(), 12);

        // But each correlation ID only has 6
        assert_eq!(sink.events_for(corr_mag).len(), 6);
        assert_eq!(sink.events_for(corr_zinc).len(), 6);
    }
}
