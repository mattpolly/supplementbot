use event_log::events::{
    CurriculumStage, EdgeRef, MutationOp, NodeRef, PipelineEvent,
    TokenUsage as EventTokenUsage,
};
use event_log::sink::EventSink;
use graph_service::graph::KnowledgeGraph;
use graph_service::lens::ComplexityLens;
use graph_service::types::*;
use llm_client::provider::{CompletionRequest, LlmProvider};
use uuid::Uuid;

use crate::prompt;

// ---------------------------------------------------------------------------
// Extraction result
// ---------------------------------------------------------------------------

/// Summary of what one extraction pass produced
#[derive(Debug, Clone)]
pub struct ExtractionSummary {
    pub nodes_added: Vec<NodeRef>,
    pub edges_added: Vec<EdgeRef>,
    /// Edges that matched existing edges in the graph (deduplicated)
    pub edges_confirmed: Vec<EdgeRef>,
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// ExtractionParser
// ---------------------------------------------------------------------------

pub struct ExtractionParser<'a> {
    provider: &'a dyn LlmProvider,
    sink: &'a dyn EventSink,
    iteration: u32,
    epoch: u32,
    lens: ComplexityLens,
}

impl<'a> ExtractionParser<'a> {
    pub fn new(
        provider: &'a dyn LlmProvider,
        sink: &'a dyn EventSink,
        iteration: u32,
        epoch: u32,
    ) -> Self {
        Self {
            provider,
            sink,
            iteration,
            epoch,
            lens: ComplexityLens::default(),
        }
    }

    /// Set the complexity lens for this parser
    pub fn with_lens(mut self, lens: ComplexityLens) -> Self {
        self.lens = lens;
        self
    }

    /// Extract triples from a single sentence and write them into the graph.
    ///
    /// The complexity lens gates which node/edge types the LLM is told about
    /// (via the prompt) and which triples are accepted (via the parser).
    pub async fn extract_sentence(
        &self,
        nutraceutical: &str,
        sentence: &str,
        stage: CurriculumStage,
        graph: &mut KnowledgeGraph,
        correlation_id: Uuid,
    ) -> ExtractionSummary {
        // Emit extraction input event
        self.sink.emit(
            correlation_id,
            PipelineEvent::ExtractionInput {
                raw_response: sentence.to_string(),
                nutraceutical: nutraceutical.to_string(),
                stage: stage.clone(),
            },
        );

        // Collect existing node names with types so the LLM can reuse them
        let existing_vocab: Vec<String> = graph
            .all_nodes()
            .iter()
            .filter_map(|&idx| {
                graph.node_data(idx).map(|n| format!("{} ({:?})", n.name, n.node_type))
            })
            .collect();
        let existing_refs: Vec<&str> = existing_vocab.iter().map(|s| s.as_str()).collect();

        // Ask LLM for structured triples (prompt is lens-aware + vocabulary-aware)
        let user_prompt = prompt::extraction_prompt(nutraceutical, sentence);
        let request = CompletionRequest::new(&user_prompt)
            .with_system(prompt::extraction_system(&self.lens, &existing_refs))
            .with_temperature(0.0);

        let raw_response = match self.provider.complete(request).await {
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
                response.content
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
                return ExtractionSummary {
                    nodes_added: Vec::new(),
                    edges_added: Vec::new(),
                    edges_confirmed: Vec::new(),
                    warnings: vec![format!("LLM extraction failed: {}", e)],
                };
            }
        };

        // Parse the response into triples (lens enforces type visibility)
        let (triples, mut warnings) = prompt::parse_triples(&raw_response, Some(&self.lens));

        // Assign confidence based on stage
        let base_confidence = match stage {
            CurriculumStage::Foundational => 0.7,
            CurriculumStage::Relational => 0.85,
        };

        // Write triples into the graph
        let mut nodes_added = Vec::new();
        let mut edges_added = Vec::new();
        let mut edges_confirmed = Vec::new();

        for triple in &triples {
            let (src_ref, src_idx) =
                self.ensure_node(graph, &triple.subject_name, &triple.subject_type, correlation_id);
            let (tgt_ref, tgt_idx) =
                self.ensure_node(graph, &triple.object_name, &triple.object_type, correlation_id);

            // Re-validate type pair against *stored* node types (may differ from parsed types
            // when a node already existed with a different type)
            let stored_src_type = &graph.node_data(src_idx).unwrap().node_type;
            let stored_tgt_type = &graph.node_data(tgt_idx).unwrap().node_type;
            if !triple.edge_type.is_valid_pair(stored_src_type, stored_tgt_type) {
                warnings.push(format!(
                    "type pair {:?}→{:?} invalid for edge {:?}, skipping: \"{}|{:?}|{}|{}|{:?}\"",
                    stored_src_type, stored_tgt_type, triple.edge_type,
                    triple.subject_name, triple.subject_type,
                    triple.edge_type, triple.object_name, triple.object_type,
                ));
                continue;
            }

            if let Some(r) = src_ref {
                nodes_added.push(r);
            }
            if let Some(r) = tgt_ref {
                nodes_added.push(r);
            }

            let edge_ref = EdgeRef {
                source: triple.subject_name.clone(),
                target: triple.object_name.clone(),
                edge_type: format!("{}", triple.edge_type),
                confidence: base_confidence,
            };

            // Check for duplicate edge before adding
            if !self.edge_exists(graph, src_idx, tgt_idx, &triple.edge_type) {
                let metadata = EdgeMetadata::extracted(base_confidence, self.iteration, self.epoch);
                graph.add_edge(
                    src_idx,
                    tgt_idx,
                    EdgeData::new(triple.edge_type.clone(), metadata),
                );

                self.sink.emit(
                    correlation_id,
                    PipelineEvent::GraphEdgeMutation {
                        operation: MutationOp::Added,
                        source_node: triple.subject_name.clone(),
                        target_node: triple.object_name.clone(),
                        edge_type: format!("{}", triple.edge_type),
                        confidence: base_confidence,
                    },
                );

                edges_added.push(edge_ref);
            } else {
                edges_confirmed.push(edge_ref);
            }
        }

        // Emit extraction output event
        self.sink.emit(
            correlation_id,
            PipelineEvent::ExtractionOutput {
                nodes_added: nodes_added.clone(),
                edges_added: edges_added.clone(),
                parse_warnings: warnings.clone(),
            },
        );

        if triples.is_empty() && warnings.is_empty() {
            warnings.push("no triples extracted from response".to_string());
        }

        ExtractionSummary {
            nodes_added,
            edges_added,
            edges_confirmed,
            warnings,
        }
    }

    /// Add a node to the graph if it doesn't exist yet.
    fn ensure_node(
        &self,
        graph: &mut KnowledgeGraph,
        name: &str,
        node_type: &NodeType,
        correlation_id: Uuid,
    ) -> (Option<NodeRef>, graph_service::graph::NodeIndex) {
        let already_exists = graph.find_node(name).is_some();
        let idx = graph.add_node(NodeData::new(name, node_type.clone()));

        if already_exists {
            (None, idx)
        } else {
            let node_ref = NodeRef {
                name: name.to_string(),
                node_type: format!("{:?}", node_type),
            };

            self.sink.emit(
                correlation_id,
                PipelineEvent::GraphNodeMutation {
                    operation: MutationOp::Added,
                    node_name: name.to_string(),
                    node_type: format!("{:?}", node_type),
                },
            );

            (Some(node_ref), idx)
        }
    }

    /// Check if an edge with the same type already exists between two nodes
    fn edge_exists(
        &self,
        graph: &KnowledgeGraph,
        source: graph_service::graph::NodeIndex,
        target: graph_service::graph::NodeIndex,
        edge_type: &EdgeType,
    ) -> bool {
        graph
            .outgoing_edges(source)
            .iter()
            .any(|(tgt, data)| *tgt == target && data.edge_type == *edge_type)
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
            .on(
                "Extract graph triples",
                "magnesium|Ingredient|affords|muscle relaxation|Property\n\
                 magnesium|Ingredient|acts_on|muscular system|System\n\
                 magnesium|Ingredient|affords|sleep quality|Property",
            )
            .with_default(
                "magnesium|Ingredient|modulates|nervous system|System",
            )
    }

    #[tokio::test]
    async fn test_extract_sentence_adds_nodes_and_edges() {
        let provider = mock_provider();
        let sink = MemorySink::new();
        let parser = ExtractionParser::new(&provider, &sink, 1, 0);
        let mut graph = KnowledgeGraph::new();
        let corr_id = Uuid::new_v4();

        let summary = parser
            .extract_sentence(
                "Magnesium",
                "Magnesium helps your muscles relax and helps you sleep better.",
                CurriculumStage::Foundational,
                &mut graph,
                corr_id,
            )
            .await;

        assert_eq!(graph.node_count(), 4);
        assert_eq!(graph.edge_count(), 3);
        assert_eq!(summary.edges_added.len(), 3);
        assert!(summary.warnings.is_empty());
    }

    #[tokio::test]
    async fn test_extract_does_not_duplicate_nodes() {
        let provider = mock_provider();
        let sink = MemorySink::new();
        let parser = ExtractionParser::new(&provider, &sink, 1, 0);
        let mut graph = KnowledgeGraph::new();
        let corr_id = Uuid::new_v4();

        parser
            .extract_sentence(
                "Magnesium",
                "Magnesium helps your muscles relax and helps you sleep better.",
                CurriculumStage::Foundational,
                &mut graph,
                corr_id,
            )
            .await;

        parser
            .extract_sentence(
                "Magnesium",
                "Magnesium helps your muscles relax and helps you sleep better.",
                CurriculumStage::Foundational,
                &mut graph,
                corr_id,
            )
            .await;

        assert_eq!(graph.node_count(), 4);
        assert_eq!(graph.edge_count(), 3);
    }

    #[tokio::test]
    async fn test_extract_reports_confirmed_edges() {
        let provider = mock_provider();
        let sink = MemorySink::new();
        let parser = ExtractionParser::new(&provider, &sink, 1, 0);
        let mut graph = KnowledgeGraph::new();
        let corr_id = Uuid::new_v4();

        // First extraction — all edges are new
        let first = parser
            .extract_sentence(
                "Magnesium",
                "Magnesium helps your muscles relax and helps you sleep better.",
                CurriculumStage::Foundational,
                &mut graph,
                corr_id,
            )
            .await;

        assert_eq!(first.edges_added.len(), 3);
        assert_eq!(first.edges_confirmed.len(), 0);

        // Second extraction of same content — all edges should be confirmed
        let second = parser
            .extract_sentence(
                "Magnesium",
                "Magnesium helps your muscles relax and helps you sleep better.",
                CurriculumStage::Foundational,
                &mut graph,
                corr_id,
            )
            .await;

        assert_eq!(second.edges_added.len(), 0, "no new edges on repeat");
        assert_eq!(second.edges_confirmed.len(), 3, "all edges confirmed on repeat");
    }

    #[tokio::test]
    async fn test_extract_accumulates_across_sentences() {
        let provider = MockProvider::new("mock", "mock-v1")
            .on(
                "muscles relax",
                "magnesium|Ingredient|affords|muscle relaxation|Property\n\
                 magnesium|Ingredient|acts_on|muscular system|System",
            )
            .on(
                "nervous system",
                "magnesium|Ingredient|modulates|nervous system|System",
            )
            .with_default("magnesium|Ingredient|affords|general health|Property");

        let sink = MemorySink::new();
        let parser = ExtractionParser::new(&provider, &sink, 1, 0);
        let mut graph = KnowledgeGraph::new();
        let corr_id = Uuid::new_v4();

        parser
            .extract_sentence(
                "Magnesium",
                "Magnesium helps your muscles relax.",
                CurriculumStage::Foundational,
                &mut graph,
                corr_id,
            )
            .await;

        parser
            .extract_sentence(
                "Magnesium",
                "Magnesium calms the nervous system.",
                CurriculumStage::Foundational,
                &mut graph,
                corr_id,
            )
            .await;

        assert!(graph.find_node("nervous system").is_some());
        assert_eq!(graph.node_count(), 4);
        assert_eq!(graph.edge_count(), 3);
    }

    #[tokio::test]
    async fn test_confidence_varies_by_stage() {
        let provider = MockProvider::new("mock", "mock-v1")
            .with_default("magnesium|Ingredient|acts_on|nervous system|System");
        let sink = MemorySink::new();
        let mut graph = KnowledgeGraph::new();
        let corr_id = Uuid::new_v4();

        let parser = ExtractionParser::new(&provider, &sink, 1, 0);
        parser
            .extract_sentence(
                "Magnesium",
                "sentence one",
                CurriculumStage::Foundational,
                &mut graph,
                corr_id,
            )
            .await;

        let mag_idx = graph.find_node("magnesium").unwrap();
        let edges = graph.outgoing_edges(mag_idx);
        assert!((edges[0].1.metadata.confidence - 0.7).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_events_emitted() {
        let provider = mock_provider();
        let sink = MemorySink::new();
        let parser = ExtractionParser::new(&provider, &sink, 1, 0);
        let mut graph = KnowledgeGraph::new();
        let corr_id = Uuid::new_v4();

        parser
            .extract_sentence(
                "Magnesium",
                "Magnesium helps your muscles relax.",
                CurriculumStage::Foundational,
                &mut graph,
                corr_id,
            )
            .await;

        let events = sink.events_for(corr_id);

        let has_input = events
            .iter()
            .any(|e| matches!(e.event, PipelineEvent::ExtractionInput { .. }));
        let has_output = events
            .iter()
            .any(|e| matches!(e.event, PipelineEvent::ExtractionOutput { .. }));

        assert!(has_input, "should emit ExtractionInput");
        assert!(has_output, "should emit ExtractionOutput");
    }

    #[tokio::test]
    async fn test_lens_filters_extraction() {
        // Mock returns a triple with Substrate — should be rejected at 5th grade
        let provider = MockProvider::new("mock", "mock-v1")
            .with_default("magnesium|Ingredient|competes_with|calcium|Substrate");

        let sink = MemorySink::new();
        let parser = ExtractionParser::new(&provider, &sink, 1, 0)
            .with_lens(ComplexityLens::fifth_grade());
        let mut graph = KnowledgeGraph::new();
        let corr_id = Uuid::new_v4();

        let summary = parser
            .extract_sentence(
                "Magnesium",
                "something",
                CurriculumStage::Foundational,
                &mut graph,
                corr_id,
            )
            .await;

        // Should have rejected the triple
        assert_eq!(graph.edge_count(), 0);
        assert!(
            summary.warnings.iter().any(|w| w.contains("exceeds complexity")),
            "should warn about complexity violation"
        );
    }
}
