use event_log::events::{
    CurriculumStage, PipelineEvent, TokenUsage as EventTokenUsage,
};
use event_log::sink::EventSink;
use extraction::parser::ExtractionParser;
use graph_service::graph::KnowledgeGraph;
use llm_client::provider::{CompletionRequest, LlmProvider};
use uuid::Uuid;

use crate::prompts;
use crate::structural;

// ---------------------------------------------------------------------------
// Speculative inference — validate structural observations with the LLM
// ---------------------------------------------------------------------------

/// Result of a speculative inference pass
#[derive(Debug, Clone)]
pub struct SpeculativeResult {
    pub observations_found: usize,
    pub observations_validated: usize,
    pub edges_added: usize,
}

/// Run speculative inference: find structural patterns, ask the LLM to validate,
/// extract new edges tagged as StructurallyEmergent.
///
/// Only runs when the graph has 2+ ingredients. Caps at `max_observations`
/// to prevent runaway LLM calls.
pub async fn run_speculative_inference(
    provider: &dyn LlmProvider,
    sink: &dyn EventSink,
    parser: &ExtractionParser<'_>,
    graph: &KnowledgeGraph,
    max_observations: usize,
    correlation_id: Uuid,
) -> SpeculativeResult {
    let observations = structural::find_observations(graph).await;

    if observations.is_empty() {
        return SpeculativeResult {
            observations_found: 0,
            observations_validated: 0,
            edges_added: 0,
        };
    }

    let observations_found = observations.len();
    let mut observations_validated = 0;
    let mut total_edges_added = 0;

    for obs in observations.iter().take(max_observations) {
        // Emit the speculative claim event
        sink.emit(
            correlation_id,
            PipelineEvent::SpeculativeClaim {
                claim: obs.description.clone(),
                topology_justification: format!("{:?}", obs.kind),
                source_nodes: obs.involved.clone(),
            },
        );

        // Generate the validation question
        let question = prompts::speculative_question(obs);
        let request = CompletionRequest::new(&question)
            .with_system(prompts::speculative_system_prompt().to_string());

        sink.emit(
            correlation_id,
            PipelineEvent::LlmRequest {
                provider: provider.provider_name().to_string(),
                model: provider.model_name().to_string(),
                prompt: question.clone(),
                nutraceutical: obs.involved.first().cloned().unwrap_or_default(),
                stage: CurriculumStage::Foundational,
                question_type: format!("speculative:{:?}", obs.kind),
            },
        );

        let response = match provider.complete(request).await {
            Ok(response) => {
                sink.emit(
                    correlation_id,
                    PipelineEvent::LlmResponse {
                        provider: provider.provider_name().to_string(),
                        model: provider.model_name().to_string(),
                        raw_response: response.content.clone(),
                        latency_ms: response.latency_ms,
                        tokens_used: response.usage.map(|u| EventTokenUsage {
                            input_tokens: u.input_tokens,
                            output_tokens: u.output_tokens,
                        }),
                    },
                );
                response.content
            }
            Err(e) => {
                sink.emit(
                    correlation_id,
                    PipelineEvent::LlmError {
                        provider: provider.provider_name().to_string(),
                        model: provider.model_name().to_string(),
                        error: e.to_string(),
                    },
                );
                continue;
            }
        };

        // Use the first ingredient as the nutraceutical context for extraction
        let nutraceutical = obs.involved.first().map(|s| s.as_str()).unwrap_or("supplement");

        let extraction = parser
            .extract_sentence(
                nutraceutical,
                &response,
                CurriculumStage::Foundational,
                graph,
                correlation_id,
            )
            .await;

        if !extraction.edges_added.is_empty() {
            observations_validated += 1;
            total_edges_added += extraction.edges_added.len();
        }
    }

    SpeculativeResult {
        observations_found,
        observations_validated,
        edges_added: total_edges_added,
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
    use graph_service::types::*;
    use llm_client::mock::MockProvider;

    #[tokio::test]
    async fn test_no_inference_with_single_ingredient() {
        let provider = MockProvider::new("mock", "mock-v1")
            .with_default("magnesium|Ingredient|affords|health|Property");
        let sink = MemorySink::new();
        let parser = ExtractionParser::new(&provider, &sink, 1, 0);
        let graph = KnowledgeGraph::in_memory().await.unwrap();

        let mag = graph.add_node(NodeData::new("magnesium", NodeType::Ingredient)).await;
        let sys = graph.add_node(NodeData::new("muscular system", NodeType::System)).await;
        graph.add_edge(&mag, &sys, EdgeData::new(EdgeType::ActsOn, EdgeMetadata::extracted(0.7, 1, 0))).await;

        let corr_id = Uuid::new_v4();
        let result = run_speculative_inference(&provider, &sink, &parser, &graph, 3, corr_id).await;

        assert_eq!(result.observations_found, 0);
        assert_eq!(result.edges_added, 0);
    }

    #[tokio::test]
    async fn test_inference_processes_shared_system() {
        let provider = MockProvider::new("mock", "mock-v1")
            // Response to speculative question about shared muscular system
            // Use "does anything special" which appears in the speculative question
            // but NOT in the extraction prompt
            .on(
                "does anything special",
                "Taking magnesium and zinc helps your muscles work better.",
            )
            // Extraction of that response — match on the extraction format instruction
            // which only appears in extraction prompts, not speculative questions
            .with_default("magnesium|Ingredient|affords|muscle function|Property");

        let sink = MemorySink::new();
        let parser = ExtractionParser::new(&provider, &sink, 1, 0)
            .with_source(Source::StructurallyEmergent)
            .with_confidence(0.5);
        let graph = KnowledgeGraph::in_memory().await.unwrap();

        // Build a two-ingredient graph with a shared system
        let mag = graph.add_node(NodeData::new("magnesium", NodeType::Ingredient)).await;
        let zinc = graph.add_node(NodeData::new("zinc", NodeType::Ingredient)).await;
        let muscular = graph.add_node(NodeData::new("muscular system", NodeType::System)).await;
        let meta = EdgeMetadata::extracted(0.7, 1, 0);
        graph.add_edge(&mag, &muscular, EdgeData::new(EdgeType::ActsOn, meta.clone())).await;
        graph.add_edge(&zinc, &muscular, EdgeData::new(EdgeType::ActsOn, meta.clone())).await;

        let corr_id = Uuid::new_v4();
        let result = run_speculative_inference(&provider, &sink, &parser, &graph, 3, corr_id).await;

        assert!(result.observations_found >= 1, "should find shared system observation");
        assert!(result.edges_added >= 1, "should add speculative edges");
    }

    #[tokio::test]
    async fn test_speculative_emits_claim_event() {
        let provider = MockProvider::new("mock", "mock-v1")
            .with_default("magnesium|Ingredient|affords|general health|Property");
        let sink = MemorySink::new();
        let parser = ExtractionParser::new(&provider, &sink, 1, 0);
        let graph = KnowledgeGraph::in_memory().await.unwrap();

        let mag = graph.add_node(NodeData::new("magnesium", NodeType::Ingredient)).await;
        let zinc = graph.add_node(NodeData::new("zinc", NodeType::Ingredient)).await;
        let sys = graph.add_node(NodeData::new("immune system", NodeType::System)).await;
        let meta = EdgeMetadata::extracted(0.7, 1, 0);
        graph.add_edge(&mag, &sys, EdgeData::new(EdgeType::ActsOn, meta.clone())).await;
        graph.add_edge(&zinc, &sys, EdgeData::new(EdgeType::ActsOn, meta.clone())).await;

        let corr_id = Uuid::new_v4();
        run_speculative_inference(&provider, &sink, &parser, &graph, 3, corr_id).await;

        let events = sink.events_for(corr_id);
        let has_claim = events
            .iter()
            .any(|e| matches!(e.event, PipelineEvent::SpeculativeClaim { .. }));
        assert!(has_claim, "should emit SpeculativeClaim event");
    }
}
