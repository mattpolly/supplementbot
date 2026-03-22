use event_log::events::{
    CurriculumStage, GapInfo, PipelineEvent, TokenUsage as EventTokenUsage,
};
use event_log::sink::EventSink;
use extraction::parser::ExtractionParser;
use graph_service::graph::KnowledgeGraph;
use graph_service::lens::ComplexityLens;
use graph_service::source::SourceStore;
use graph_service::types::Source;
use llm_client::provider::{CompletionRequest, LlmProvider};
use uuid::Uuid;

use crate::analyzer;
use crate::comprehension;
use crate::forward_chain;
use crate::prompts;
use crate::speculative;

// ---------------------------------------------------------------------------
// Loop configuration
// ---------------------------------------------------------------------------

/// How the loop behaves
pub struct LoopConfig {
    /// Max gap-filling iterations before moving to comprehension check
    pub max_gap_iterations: u32,
    /// Max gaps to fill per iteration (prevents runaway)
    pub max_gaps_per_iteration: usize,
    /// Max structural observations to validate with the LLM (0 = skip speculative)
    pub max_speculative_observations: usize,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_gap_iterations: 3,
            max_gaps_per_iteration: 5,
            max_speculative_observations: 3,
        }
    }
}

// ---------------------------------------------------------------------------
// Loop result
// ---------------------------------------------------------------------------

/// Summary of what the NSAI loop did
#[derive(Debug, Clone)]
pub struct LoopResult {
    pub iterations: u32,
    pub total_gaps_filled: usize,
    pub deduced_chains: usize,
    pub deduced_edges_added: usize,
    pub comprehension_edges_confirmed: usize,
    pub comprehension_edges_new: usize,
    pub speculative_observations: usize,
    pub speculative_edges_added: usize,
    pub final_node_count: usize,
    pub final_edge_count: usize,
}

// ---------------------------------------------------------------------------
// NsaiLoop
// ---------------------------------------------------------------------------

pub struct NsaiLoop<'a> {
    provider: &'a dyn LlmProvider,
    sink: &'a dyn EventSink,
    config: LoopConfig,
    lens: ComplexityLens,
    source_store: Option<&'a SourceStore>,
}

impl<'a> NsaiLoop<'a> {
    pub fn new(provider: &'a dyn LlmProvider, sink: &'a dyn EventSink) -> Self {
        Self {
            provider,
            sink,
            config: LoopConfig::default(),
            lens: ComplexityLens::default(),
            source_store: None,
        }
    }

    pub fn with_config(mut self, config: LoopConfig) -> Self {
        self.config = config;
        self
    }

    pub fn with_lens(mut self, lens: ComplexityLens) -> Self {
        self.lens = lens;
        self
    }

    pub fn with_source_store(mut self, store: &'a SourceStore) -> Self {
        self.source_store = Some(store);
        self
    }

    /// Run the full NSAI loop for a nutraceutical at 5th grade level.
    ///
    /// 1. Seed question → extract into graph
    /// 2. Analyze gaps → fill gaps (repeat until stable or max iterations)
    /// 3. Comprehension check → re-extract → compare
    pub async fn run(
        &self,
        nutraceutical: &str,
        graph: &KnowledgeGraph,
        correlation_id: Uuid,
    ) -> LoopResult {
        let mut parser = ExtractionParser::new(self.provider, self.sink, 1, 0)
            .with_lens(self.lens);
        if let Some(store) = self.source_store {
            parser = parser.with_source_store(store);
        }
        let mut total_gaps_filled = 0usize;
        let mut iteration = 0u32;

        // ── Step 1: Seed question ──────────────────────────────────────────
        let seed_answer = self.ask_seed(nutraceutical, correlation_id).await;

        if let Some(answer) = seed_answer {
            let nodes_before = graph.node_count().await;
            let edges_before = graph.edge_count().await;

            parser
                .extract_sentence(
                    nutraceutical,
                    &answer,
                    CurriculumStage::Foundational,
                    graph,
                    correlation_id,
                )
                .await;

            self.sink.emit(
                correlation_id,
                PipelineEvent::LoopIteration {
                    iteration: 0,
                    phase: "seed".to_string(),
                    gaps_found: 0,
                    nodes_before,
                    nodes_after: graph.node_count().await,
                    edges_before,
                    edges_after: graph.edge_count().await,
                },
            );
        }

        // ── Step 2: Gap-filling loop ───────────────────────────────────────
        for i in 1..=self.config.max_gap_iterations {
            iteration = i;

            let gaps = analyzer::find_gaps(graph, nutraceutical).await;
            if gaps.is_empty() {
                break;
            }

            // Emit gap analysis event
            self.sink.emit(
                correlation_id,
                PipelineEvent::GapAnalysis {
                    gaps: gaps
                        .iter()
                        .map(|g| GapInfo {
                            node_name: g.node_name.clone(),
                            gap_type: g.kind.label().to_string(),
                            description: g.kind.description(&g.node_name),
                        })
                        .collect(),
                    graph_nodes: graph.node_count().await,
                    graph_edges: graph.edge_count().await,
                },
            );

            let nodes_before = graph.node_count().await;
            let edges_before = graph.edge_count().await;
            let gaps_this_round = gaps.len().min(self.config.max_gaps_per_iteration);

            for gap in gaps.iter().take(self.config.max_gaps_per_iteration) {
                let question = prompts::gap_question(nutraceutical, gap);

                let request = CompletionRequest::new(&question)
                    .with_system(prompts::gap_system_prompt().to_string());

                self.sink.emit(
                    correlation_id,
                    PipelineEvent::LlmRequest {
                        provider: self.provider.provider_name().to_string(),
                        model: self.provider.model_name().to_string(),
                        prompt: question.clone(),
                        nutraceutical: nutraceutical.to_string(),
                        stage: CurriculumStage::Foundational,
                        question_type: format!("gap_fill:{}", gap.kind.label()),
                    },
                );

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

                        parser
                            .extract_sentence(
                                nutraceutical,
                                &response.content,
                                CurriculumStage::Foundational,
                                graph,
                                correlation_id,
                            )
                            .await;
                    }
                    Err(e) => {
                        self.sink.emit(
                            correlation_id,
                            PipelineEvent::LlmError {
                                provider: self.provider.provider_name().to_string(),
                                model: self.provider.model_name().to_string(),
                                error: e.to_string(),
                            },
                        );
                    }
                }
            }

            total_gaps_filled += gaps_this_round;

            let nodes_after = graph.node_count().await;
            let edges_after = graph.edge_count().await;

            self.sink.emit(
                correlation_id,
                PipelineEvent::LoopIteration {
                    iteration: i,
                    phase: "gap_fill".to_string(),
                    gaps_found: gaps.len(),
                    nodes_before,
                    nodes_after,
                    edges_before,
                    edges_after,
                },
            );

            // If nothing changed, stop early
            if nodes_after == nodes_before && edges_after == edges_before {
                break;
            }
        }

        // ── Step 3: Forward chaining (symbolic deduction) ─────────────────
        let chain_result = forward_chain::run_forward_chaining(
            self.sink,
            graph,
            correlation_id,
        )
        .await;

        // ── Step 4: Comprehension check ────────────────────────────────────
        let comp = comprehension::check_comprehension(
            self.provider,
            self.sink,
            &parser,
            graph,
            nutraceutical,
            correlation_id,
        )
        .await;

        self.sink.emit(
            correlation_id,
            PipelineEvent::LoopIteration {
                iteration: iteration + 1,
                phase: "comprehension".to_string(),
                gaps_found: 0,
                nodes_before: graph.node_count().await,
                nodes_after: graph.node_count().await,
                edges_before: comp.edges_total.saturating_sub(comp.edges_new),
                edges_after: comp.edges_total,
            },
        );

        // ── Step 4: Speculative inference ────────────────────────────────
        let mut speculative_observations = 0;
        let mut speculative_edges_added = 0;

        if self.config.max_speculative_observations > 0 {
            let mut spec_parser = ExtractionParser::new(self.provider, self.sink, 1, 0)
                .with_lens(self.lens)
                .with_source(Source::StructurallyEmergent)
                .with_confidence(0.5);
            if let Some(store) = self.source_store {
                spec_parser = spec_parser.with_source_store(store);
            }

            let spec = speculative::run_speculative_inference(
                self.provider,
                self.sink,
                &spec_parser,
                graph,
                self.config.max_speculative_observations,
                correlation_id,
            )
            .await;

            speculative_observations = spec.observations_found;
            speculative_edges_added = spec.edges_added;
        }

        LoopResult {
            iterations: iteration,
            total_gaps_filled,
            deduced_chains: chain_result.chains_found,
            deduced_edges_added: chain_result.edges_added,
            comprehension_edges_confirmed: comp.edges_confirmed,
            comprehension_edges_new: comp.edges_new,
            speculative_observations,
            speculative_edges_added,
            final_node_count: graph.node_count().await,
            final_edge_count: graph.edge_count().await,
        }
    }

    /// Ask the seed question: "What does X do as a supplement?" at 5th grade level.
    async fn ask_seed(&self, nutraceutical: &str, correlation_id: Uuid) -> Option<String> {
        let prompt = format!(
            "Explain to a 5th grader (10 years old) what {} does as a supplement, \
             in one sentence. Use simple everyday words. No scientific terms.",
            nutraceutical
        );

        self.sink.emit(
            correlation_id,
            PipelineEvent::LlmRequest {
                provider: self.provider.provider_name().to_string(),
                model: self.provider.model_name().to_string(),
                prompt: prompt.clone(),
                nutraceutical: nutraceutical.to_string(),
                stage: CurriculumStage::Foundational,
                question_type: "seed".to_string(),
            },
        );

        let request = CompletionRequest::new(&prompt)
            .with_system(prompts::gap_system_prompt().to_string());

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
                Some(response.content)
            }
            Err(e) => {
                self.sink.emit(
                    correlation_id,
                    PipelineEvent::LlmError {
                        provider: self.provider.provider_name().to_string(),
                        model: self.provider.model_name().to_string(),
                        error: e.to_string(),
                    },
                );
                None
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
    use event_log::sink::MemorySink;
    use graph_service::graph::KnowledgeGraph;
    use llm_client::mock::MockProvider;

    fn loop_mock() -> MockProvider {
        MockProvider::new("mock", "mock-v1")
            // Seed answer
            .on(
                "5th grader",
                "Magnesium helps your muscles relax and helps you sleep better.",
            )
            // Extraction of seed
            .on(
                "muscles relax",
                "magnesium|Ingredient|affords|muscle relaxation|Property\n\
                 magnesium|Ingredient|acts_on|muscular system|System\n\
                 magnesium|Ingredient|affords|sleep quality|Property",
            )
            // Gap fill: "why is magnesium connected to muscle relaxation"
            .on(
                "connected to muscle relaxation",
                "Magnesium helps muscles relax by stopping them from staying tight.",
            )
            // Extraction of gap-fill answer
            .on(
                "staying tight",
                "magnesium|Ingredient|via_mechanism|muscle tension relief|Mechanism\n\
                 muscle tension relief|Mechanism|affords|muscle relaxation|Property",
            )
            // Gap fill: "how does magnesium help with sleep quality"
            .on(
                "help with sleep quality",
                "Magnesium helps calm your brain so you can fall asleep.",
            )
            // Extraction of sleep gap-fill
            .on(
                "calm your brain",
                "magnesium|Ingredient|acts_on|nervous system|System\n\
                 magnesium|Ingredient|affords|sleep quality|Property",
            )
            // Comprehension rephrase
            .on(
                "explain the same",
                "Magnesium keeps your muscles from getting too tight and helps your brain relax for sleep.",
            )
            // Extraction of rephrase (same structure)
            .on(
                "too tight",
                "magnesium|Ingredient|affords|muscle relaxation|Property\n\
                 magnesium|Ingredient|affords|sleep quality|Property",
            )
            // Catch-all for any other extraction
            .with_default("magnesium|Ingredient|affords|general health|Property")
    }

    #[tokio::test]
    async fn test_loop_runs_seed_and_gaps() {
        let provider = loop_mock();
        let sink = MemorySink::new();
        let nsai = NsaiLoop::new(&provider, &sink);
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let corr_id = Uuid::new_v4();

        let result = nsai.run("Magnesium", &graph, corr_id).await;

        // Should have created nodes
        assert!(graph.node_count().await > 0, "graph should have nodes");
        assert!(graph.edge_count().await > 0, "graph should have edges");
        // Should have done at least 1 gap-filling iteration
        assert!(result.iterations >= 1);
    }

    #[tokio::test]
    async fn test_loop_emits_events() {
        let provider = loop_mock();
        let sink = MemorySink::new();
        let nsai = NsaiLoop::new(&provider, &sink);
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let corr_id = Uuid::new_v4();

        nsai.run("Magnesium", &graph, corr_id).await;

        let events = sink.events_for(corr_id);

        let has_loop_iter = events
            .iter()
            .any(|e| matches!(e.event, PipelineEvent::LoopIteration { .. }));
        let has_gap_analysis = events
            .iter()
            .any(|e| matches!(e.event, PipelineEvent::GapAnalysis { .. }));
        let has_comprehension = events
            .iter()
            .any(|e| matches!(e.event, PipelineEvent::ComprehensionCheck { .. }));

        assert!(has_loop_iter, "should emit LoopIteration events");
        assert!(has_gap_analysis, "should emit GapAnalysis events");
        assert!(has_comprehension, "should emit ComprehensionCheck event");
    }

    #[tokio::test]
    async fn test_loop_with_config() {
        let provider = loop_mock();
        let sink = MemorySink::new();
        let config = LoopConfig {
            max_gap_iterations: 1,
            max_gaps_per_iteration: 2,
            max_speculative_observations: 0,
        };
        let nsai = NsaiLoop::new(&provider, &sink).with_config(config);
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let corr_id = Uuid::new_v4();

        let result = nsai.run("Magnesium", &graph, corr_id).await;

        // With max 1 iteration, should stop after first gap-fill round
        assert!(result.iterations <= 1);
    }
}
