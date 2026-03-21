use event_log::events::{PipelineEvent, TokenUsage as EventTokenUsage};
use event_log::sink::EventSink;
use extraction::parser::ExtractionParser;
use graph_service::graph::KnowledgeGraph;
use llm_client::provider::{CompletionRequest, LlmProvider};
use uuid::Uuid;

use crate::prompts;

/// Result of a comprehension check
#[derive(Debug, Clone)]
pub struct ComprehensionResult {
    /// The rephrase the LLM produced
    pub rephrase: String,
    /// How many existing edges were confirmed (re-extracted from rephrase)
    pub edges_confirmed: usize,
    /// How many new edges appeared in the rephrase (potential misunderstanding or new info)
    pub edges_new: usize,
    /// Total edges in graph after comprehension extraction
    pub edges_total: usize,
}

/// Run a comprehension check: summarize graph → ask LLM to rephrase → re-extract → compare.
pub async fn check_comprehension(
    provider: &dyn LlmProvider,
    sink: &dyn EventSink,
    parser: &ExtractionParser<'_>,
    graph: &mut KnowledgeGraph,
    nutraceutical: &str,
    correlation_id: Uuid,
) -> ComprehensionResult {
    let summary = prompts::summarize_graph_for_comprehension(graph, nutraceutical);

    if summary.is_empty() {
        return ComprehensionResult {
            rephrase: String::new(),
            edges_confirmed: 0,
            edges_new: 0,
            edges_total: graph.edge_count(),
        };
    }

    let prompt = prompts::comprehension_prompt(nutraceutical, &summary);
    let request = CompletionRequest::new(&prompt)
        .with_system(prompts::comprehension_system_prompt().to_string())
        .with_temperature(0.3); // slight variation for rephrasing

    let edges_before = graph.edge_count();

    let rephrase = match provider.complete(request).await {
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
            return ComprehensionResult {
                rephrase: String::new(),
                edges_confirmed: 0,
                edges_new: 0,
                edges_total: edges_before,
            };
        }
    };

    // Re-extract from the rephrase — new edges = divergence, no new edges = consistency
    let extraction = parser
        .extract_sentence(
            nutraceutical,
            &rephrase,
            event_log::events::CurriculumStage::Foundational,
            graph,
            correlation_id,
        )
        .await;

    let edges_after = graph.edge_count();
    let edges_new = extraction.edges_added.len();
    let edges_confirmed = extraction.edges_confirmed.len();

    // Emit comprehension check event
    sink.emit(
        correlation_id,
        PipelineEvent::ComprehensionCheck {
            rephrase_prompt: prompt.clone(),
            rephrase_response: rephrase.clone(),
            edges_confirmed,
            edges_new,
            edges_total: edges_after,
        },
    );

    ComprehensionResult {
        rephrase,
        edges_confirmed,
        edges_new,
        edges_total: edges_after,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_log::sink::MemorySink;
    use extraction::parser::ExtractionParser;
    use graph_service::graph::KnowledgeGraph;
    use graph_service::types::*;
    use llm_client::mock::MockProvider;

    #[tokio::test]
    async fn test_comprehension_stable_graph() {
        // Mock: rephrase returns something that extracts to the same triples
        let provider = MockProvider::new("mock", "mock-v1")
            // Comprehension rephrase
            .on(
                "explain the same",
                "Magnesium makes your muscles feel loose and helps you fall asleep easier.",
            )
            // Extraction of rephrase — same structure as existing graph
            .on(
                "muscles feel loose",
                "magnesium|Ingredient|affords|muscle relaxation|Property\n\
                 magnesium|Ingredient|affords|sleep quality|Property",
            )
            .with_default("magnesium|Ingredient|affords|general wellness|Property");

        let sink = MemorySink::new();
        let parser = ExtractionParser::new(&provider, &sink, 1, 0);
        let mut graph = KnowledgeGraph::new();

        // Pre-populate graph with existing knowledge
        let mag = graph.add_node(NodeData::new("magnesium", NodeType::Ingredient));
        let prop1 = graph.add_node(NodeData::new("muscle relaxation", NodeType::Property));
        let prop2 = graph.add_node(NodeData::new("sleep quality", NodeType::Property));
        graph.add_edge(
            mag,
            prop1,
            EdgeData::new(EdgeType::Affords, EdgeMetadata::extracted(0.7, 1, 0)),
        );
        graph.add_edge(
            mag,
            prop2,
            EdgeData::new(EdgeType::Affords, EdgeMetadata::extracted(0.7, 1, 0)),
        );

        let corr_id = Uuid::new_v4();
        let result = check_comprehension(
            &provider, &sink, &parser, &mut graph, "Magnesium", corr_id,
        )
        .await;

        // Rephrase should produce no new edges (same triples already in graph)
        assert_eq!(result.edges_new, 0, "stable graph should have no new edges from rephrase");
        assert!(!result.rephrase.is_empty());
    }

    #[tokio::test]
    async fn test_comprehension_emits_event() {
        let provider = MockProvider::new("mock", "mock-v1")
            .on("explain the same", "Magnesium is good for muscles.")
            .with_default("magnesium|Ingredient|acts_on|muscular system|System");

        let sink = MemorySink::new();
        let parser = ExtractionParser::new(&provider, &sink, 1, 0);
        let mut graph = KnowledgeGraph::new();
        let mag = graph.add_node(NodeData::new("magnesium", NodeType::Ingredient));
        let sys = graph.add_node(NodeData::new("muscular system", NodeType::System));
        graph.add_edge(
            mag,
            sys,
            EdgeData::new(EdgeType::ActsOn, EdgeMetadata::extracted(0.7, 1, 0)),
        );

        let corr_id = Uuid::new_v4();
        check_comprehension(&provider, &sink, &parser, &mut graph, "Magnesium", corr_id).await;

        let events = sink.events_for(corr_id);
        let has_check = events
            .iter()
            .any(|e| matches!(e.event, PipelineEvent::ComprehensionCheck { .. }));
        assert!(has_check, "should emit ComprehensionCheck event");
    }
}
