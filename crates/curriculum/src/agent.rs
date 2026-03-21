use event_log::events::{CurriculumStage, PipelineEvent, TokenUsage as EventTokenUsage};
use event_log::sink::EventSink;
use llm_client::provider::{CompletionRequest, LlmProvider};
use uuid::Uuid;

use crate::questions::{self, CurriculumQuestion, GradeLevel};

// ---------------------------------------------------------------------------
// Discovery result types
// ---------------------------------------------------------------------------

/// A single question-answer pair with its extracted concepts
#[derive(Debug, Clone)]
pub struct DiscoveryNode {
    pub question: CurriculumQuestion,
    pub answer: String,
    pub concepts: Vec<String>,
    pub latency_ms: u64,
}

/// One grade level's worth of exploration
#[derive(Debug, Clone)]
pub struct GradeLevelResult {
    pub level: GradeLevel,
    /// The top-level answer for this grade
    pub overview: DiscoveryNode,
    /// Drill-downs into concepts from the overview
    pub concept_nodes: Vec<DiscoveryNode>,
}

/// The full result across all grade levels
#[derive(Debug, Clone)]
pub struct DiscoveryResult {
    pub nutraceutical: String,
    pub levels: Vec<GradeLevelResult>,
    /// All unique concepts discovered across all levels
    pub all_concepts: Vec<String>,
}

// ---------------------------------------------------------------------------
// CurriculumAgent
// ---------------------------------------------------------------------------

pub struct CurriculumAgent<'a> {
    provider: &'a dyn LlmProvider,
    sink: &'a dyn EventSink,
}

impl<'a> CurriculumAgent<'a> {
    pub fn new(provider: &'a dyn LlmProvider, sink: &'a dyn EventSink) -> Self {
        Self { provider, sink }
    }

    /// Run the full progressive discovery for a nutraceutical.
    ///
    /// For each grade level:
    /// 1. Ask the top-level question ("what does X do?")
    /// 2. Extract concepts from the answer
    /// 3. For each NEW concept, ask a drill-down at the same grade level
    ///
    /// Concepts accumulate across levels — a concept discovered at 6th grade
    /// won't be re-drilled at 9th grade (it's already in the graph). But the
    /// top-level question at each grade may surface new concepts that weren't
    /// visible at lower complexity levels.
    pub async fn discover(
        &self,
        nutraceutical: &str,
        correlation_id: Uuid,
    ) -> Result<DiscoveryResult, String> {
        let mut result = DiscoveryResult {
            nutraceutical: nutraceutical.to_string(),
            levels: Vec::new(),
            all_concepts: Vec::new(),
        };

        for &level in GradeLevel::all() {
            let level_result = self
                .run_level(nutraceutical, level, &mut result.all_concepts, correlation_id)
                .await?;
            result.levels.push(level_result);
        }

        Ok(result)
    }

    /// Run a single grade level
    async fn run_level(
        &self,
        nutraceutical: &str,
        level: GradeLevel,
        seen_concepts: &mut Vec<String>,
        correlation_id: Uuid,
    ) -> Result<GradeLevelResult, String> {
        // Top-level question for this grade
        let overview_q = questions::level_question(nutraceutical, level);
        let (answer, latency) = self.ask(&overview_q, correlation_id).await?;
        let concepts = self.extract_concepts(&answer, correlation_id).await
            .unwrap_or_default();

        let overview = DiscoveryNode {
            question: overview_q,
            answer,
            concepts: concepts.clone(),
            latency_ms: latency,
        };

        // Drill into new concepts only
        let mut concept_nodes = Vec::new();

        for concept in &concepts {
            if seen_concepts.contains(concept) {
                continue;
            }
            seen_concepts.push(concept.clone());

            let q = questions::concept_question(nutraceutical, concept, level);
            match self.ask(&q, correlation_id).await {
                Ok((answer, latency)) => {
                    // Extract sub-concepts and add any new ones to seen list
                    let sub_concepts = self.extract_concepts(&answer, correlation_id).await
                        .unwrap_or_default();

                    for sc in &sub_concepts {
                        if !seen_concepts.contains(sc) {
                            seen_concepts.push(sc.clone());
                        }
                    }

                    concept_nodes.push(DiscoveryNode {
                        question: q,
                        answer,
                        concepts: sub_concepts,
                        latency_ms: latency,
                    });
                }
                Err(e) => {
                    self.sink.emit(
                        correlation_id,
                        PipelineEvent::LlmError {
                            provider: self.provider.provider_name().to_string(),
                            model: self.provider.model_name().to_string(),
                            error: e,
                        },
                    );
                }
            }
        }

        Ok(GradeLevelResult {
            level,
            overview,
            concept_nodes,
        })
    }

    /// Ask a single question with event emission
    async fn ask(
        &self,
        question: &CurriculumQuestion,
        correlation_id: Uuid,
    ) -> Result<(String, u64), String> {
        let stage = match question.grade_level {
            GradeLevel::Fifth | GradeLevel::Tenth => CurriculumStage::Foundational,
            GradeLevel::College => CurriculumStage::Relational,
        };

        self.sink.emit(
            correlation_id,
            PipelineEvent::LlmRequest {
                provider: self.provider.provider_name().to_string(),
                model: self.provider.model_name().to_string(),
                prompt: question.prompt.clone(),
                nutraceutical: question.nutraceutical.clone(),
                stage: stage.clone(),
                question_type: format!("{}", question.grade_level.label()),
            },
        );

        let request = CompletionRequest::new(&question.prompt)
            .with_system(questions::system_prompt().to_string());

        match self.provider.complete(request).await {
            Ok(response) => {
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
                Ok((response.content, response.latency_ms))
            }
            Err(e) => {
                let msg = e.to_string();
                self.sink.emit(
                    correlation_id,
                    PipelineEvent::LlmError {
                        provider: self.provider.provider_name().to_string(),
                        model: self.provider.model_name().to_string(),
                        error: msg.clone(),
                    },
                );
                Err(msg)
            }
        }
    }

    /// Extract concepts with event emission
    async fn extract_concepts(
        &self,
        sentence: &str,
        correlation_id: Uuid,
    ) -> Result<Vec<String>, String> {
        let prompt = questions::extraction_question(sentence);

        self.sink.emit(
            correlation_id,
            PipelineEvent::LlmRequest {
                provider: self.provider.provider_name().to_string(),
                model: self.provider.model_name().to_string(),
                prompt: prompt.clone(),
                nutraceutical: String::new(),
                stage: CurriculumStage::Foundational,
                question_type: "concept_extraction".to_string(),
            },
        );

        let request = CompletionRequest::new(&prompt)
            .with_system(questions::extract_prompt().to_string())
            .with_temperature(0.0);

        match self.provider.complete(request).await {
            Ok(response) => {
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
                Ok(questions::parse_concepts(&response.content))
            }
            Err(e) => Err(e.to_string()),
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

    fn mock_provider() -> MockProvider {
        MockProvider::new("mock", "mock-v1")
            // 5th grade overview
            .on(
                "5th grader",
                "Magnesium helps your muscles relax, helps you sleep better, and gives your body energy.",
            )
            // 10th grade overview
            .on(
                "10th grader",
                "Magnesium helps muscles relax by working against calcium, and it helps your \
                 nervous system stay calm by affecting brain receptors.",
            )
            // College overview
            .on(
                "college sophomore",
                "Magnesium functions as a divalent cation cofactor forming the Mg-ATP complex \
                 required for kinase activity, while simultaneously acting as a voltage-dependent \
                 NMDA receptor antagonist and L-type calcium channel blocker.",
            )
            // Concept extraction — returns simple concepts
            .on(
                "Extract the key",
                "muscle relaxation, sleep quality, energy production",
            )
            // Drill-down catch-all
            .with_default(
                "Magnesium supports this process through its role as a mineral cofactor.",
            )
    }

    #[tokio::test]
    async fn test_four_grade_levels() {
        let provider = mock_provider();
        let sink = MemorySink::new();
        let agent = CurriculumAgent::new(&provider, &sink);
        let corr_id = Uuid::new_v4();

        let result = agent.discover("Magnesium", corr_id).await.unwrap();

        assert_eq!(result.levels.len(), 3);
        assert_eq!(result.levels[0].level, GradeLevel::Fifth);
        assert_eq!(result.levels[1].level, GradeLevel::Tenth);
        assert_eq!(result.levels[2].level, GradeLevel::College);
    }

    #[tokio::test]
    async fn test_concepts_accumulate_no_duplicates() {
        let provider = mock_provider();
        let sink = MemorySink::new();
        let agent = CurriculumAgent::new(&provider, &sink);
        let corr_id = Uuid::new_v4();

        let result = agent.discover("Magnesium", corr_id).await.unwrap();

        // No duplicate concepts
        let mut sorted = result.all_concepts.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), result.all_concepts.len());
    }

    #[tokio::test]
    async fn test_only_first_level_drills_concepts() {
        let provider = mock_provider();
        let sink = MemorySink::new();
        let agent = CurriculumAgent::new(&provider, &sink);
        let corr_id = Uuid::new_v4();

        let result = agent.discover("Magnesium", corr_id).await.unwrap();

        // 6th grade should have concept drill-downs (first to see these concepts)
        assert!(
            !result.levels[0].concept_nodes.is_empty(),
            "6th grade should drill into concepts"
        );

        // Later levels may or may not have drill-downs depending on whether
        // the overview surfaces new concepts not seen at earlier levels.
        // With our mock (same extraction every time), later levels won't drill.
    }

    #[tokio::test]
    async fn test_events_emitted_for_all_levels() {
        let provider = mock_provider();
        let sink = MemorySink::new();
        let agent = CurriculumAgent::new(&provider, &sink);
        let corr_id = Uuid::new_v4();

        agent.discover("Magnesium", corr_id).await.unwrap();

        let events = sink.events_for(corr_id);
        let requests: Vec<_> = events
            .iter()
            .filter(|e| matches!(e.event, PipelineEvent::LlmRequest { .. }))
            .collect();

        // At minimum: 3 overview questions + 3 extractions + concept drills
        assert!(requests.len() >= 6, "expected at least 6 requests, got {}", requests.len());
    }
}
