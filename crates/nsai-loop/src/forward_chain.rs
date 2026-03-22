use event_log::events::PipelineEvent;
use event_log::sink::EventSink;
use graph_service::graph::{KnowledgeGraph, NodeIndex};
use graph_service::types::*;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Forward chaining — pure symbolic deduction
//
// Walk two-hop paths in the graph and deduce shortcut edges that are
// guaranteed given the premises. No LLM needed.
//
// Current deduction rules:
//   1. A →[via_mechanism]→ M →[affords]→ P  ⟹  A →[affords]→ P
//      "If A works through mechanism M, and M affords property P,
//       then A affords P."
//
//   2. A →[via_mechanism]→ M →[affords]→ P, A →[acts_on]→ S
//      (no new edge — but strengthens the acts_on relationship)
//
// Confidence for deduced edges = min(confidence of the two premise edges),
// because a chain is only as strong as its weakest link.
// ---------------------------------------------------------------------------

/// Result of a forward chaining pass
#[derive(Debug, Clone)]
pub struct ForwardChainResult {
    pub chains_found: usize,
    pub edges_added: usize,
}

/// A deduction: the two premise edges and the conclusion
#[derive(Debug, Clone)]
struct Deduction {
    /// The ingredient (or source node)
    source: NodeIndex,
    source_name: String,
    /// The intermediate mechanism
    mechanism_name: String,
    /// The target property
    target: NodeIndex,
    target_name: String,
    /// Confidence = min of the two premise confidences
    confidence: f64,
}

/// Run forward chaining over the graph. Pure symbolic — no LLM calls.
///
/// Finds all `A → via_mechanism → M → affords → P` chains where the
/// shortcut `A → affords → P` does not yet exist, and adds it.
pub async fn run_forward_chaining(
    sink: &dyn EventSink,
    graph: &KnowledgeGraph,
    correlation_id: Uuid,
) -> ForwardChainResult {
    let mut deductions = Vec::new();

    // Find all nodes that have outgoing via_mechanism edges
    let all_nodes = graph.all_nodes().await;

    for node in &all_nodes {
        let outgoing = graph.outgoing_edges(node).await;

        // Find via_mechanism edges from this node
        let mechanisms: Vec<_> = outgoing
            .iter()
            .filter(|(_, data)| data.edge_type == EdgeType::ViaMechanism)
            .collect();

        if mechanisms.is_empty() {
            continue;
        }

        // For each mechanism, check if M → affords → P exists
        for (mech_idx, mech_edge) in &mechanisms {
            let mech_outgoing = graph.outgoing_edges(mech_idx).await;

            let affordances: Vec<_> = mech_outgoing
                .iter()
                .filter(|(_, data)| data.edge_type == EdgeType::Affords)
                .collect();

            for (prop_idx, prop_edge) in &affordances {
                // Check if the shortcut A → affords → P already exists
                let shortcut_exists = outgoing.iter().any(|(tgt, data)| {
                    *tgt == *prop_idx && data.edge_type == EdgeType::Affords
                });

                if !shortcut_exists {
                    let source_data = graph.node_data(node).await;
                    let mech_data = graph.node_data(mech_idx).await;
                    let prop_data = graph.node_data(prop_idx).await;

                    if let (Some(s), Some(m), Some(p)) = (source_data, mech_data, prop_data) {
                        let confidence = mech_edge
                            .metadata
                            .confidence
                            .min(prop_edge.metadata.confidence);

                        deductions.push(Deduction {
                            source: node.clone(),
                            source_name: s.name,
                            mechanism_name: m.name,
                            target: prop_idx.clone(),
                            target_name: p.name,
                            confidence,
                        });
                    }
                }
            }
        }
    }

    let chains_found = deductions.len();
    let mut edges_added = 0;

    for ded in &deductions {
        // Add the deduced edge
        let metadata = EdgeMetadata::deduced(ded.confidence, 1, 0);
        graph
            .add_edge(
                &ded.source,
                &ded.target,
                EdgeData::new(EdgeType::Affords, metadata),
            )
            .await;
        edges_added += 1;

        // Emit event
        sink.emit(
            correlation_id,
            PipelineEvent::ForwardChain {
                rule: "via_mechanism + affords => affords".to_string(),
                premise_a: format!(
                    "{} →[via_mechanism]→ {}",
                    ded.source_name, ded.mechanism_name
                ),
                premise_b: format!(
                    "{} →[affords]→ {}",
                    ded.mechanism_name, ded.target_name
                ),
                conclusion: format!(
                    "{} →[affords]→ {}",
                    ded.source_name, ded.target_name
                ),
                confidence: ded.confidence,
            },
        );
    }

    ForwardChainResult {
        chains_found,
        edges_added,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use event_log::sink::MemorySink;

    #[tokio::test]
    async fn test_deduces_affords_from_mechanism_chain() {
        let sink = MemorySink::new();
        let graph = KnowledgeGraph::in_memory().await.unwrap();

        // Build: magnesium →[via_mechanism]→ muscle contraction regulation →[affords]→ muscle relaxation
        let mag = graph
            .add_node(NodeData::new("magnesium", NodeType::Ingredient))
            .await;
        let mech = graph
            .add_node(NodeData::new(
                "muscle contraction regulation",
                NodeType::Mechanism,
            ))
            .await;
        let prop = graph
            .add_node(NodeData::new("muscle relaxation", NodeType::Property))
            .await;

        graph
            .add_edge(
                &mag,
                &mech,
                EdgeData::new(EdgeType::ViaMechanism, EdgeMetadata::extracted(0.8, 1, 0)),
            )
            .await;
        graph
            .add_edge(
                &mech,
                &prop,
                EdgeData::new(EdgeType::Affords, EdgeMetadata::extracted(0.9, 1, 0)),
            )
            .await;

        let corr_id = Uuid::new_v4();
        let result = run_forward_chaining(&sink, &graph, corr_id).await;

        assert_eq!(result.chains_found, 1);
        assert_eq!(result.edges_added, 1);

        // Verify the deduced edge exists
        let mag_edges = graph.outgoing_edges(&mag).await;
        let has_shortcut = mag_edges.iter().any(|(tgt, data)| {
            *tgt == prop
                && data.edge_type == EdgeType::Affords
                && data.metadata.source == Source::Deduced
        });
        assert!(has_shortcut, "should have deduced affords edge");
    }

    #[tokio::test]
    async fn test_skips_existing_shortcut() {
        let sink = MemorySink::new();
        let graph = KnowledgeGraph::in_memory().await.unwrap();

        let mag = graph
            .add_node(NodeData::new("magnesium", NodeType::Ingredient))
            .await;
        let mech = graph
            .add_node(NodeData::new("calcium absorption", NodeType::Mechanism))
            .await;
        let prop = graph
            .add_node(NodeData::new("bone mineralization", NodeType::Property))
            .await;

        graph
            .add_edge(
                &mag,
                &mech,
                EdgeData::new(EdgeType::ViaMechanism, EdgeMetadata::extracted(0.7, 1, 0)),
            )
            .await;
        graph
            .add_edge(
                &mech,
                &prop,
                EdgeData::new(EdgeType::Affords, EdgeMetadata::extracted(0.8, 1, 0)),
            )
            .await;
        // Shortcut already exists
        graph
            .add_edge(
                &mag,
                &prop,
                EdgeData::new(EdgeType::Affords, EdgeMetadata::extracted(0.7, 1, 0)),
            )
            .await;

        let corr_id = Uuid::new_v4();
        let result = run_forward_chaining(&sink, &graph, corr_id).await;

        assert_eq!(result.chains_found, 0, "should not find new deductions");
        assert_eq!(result.edges_added, 0);
    }

    #[tokio::test]
    async fn test_confidence_is_weakest_link() {
        let sink = MemorySink::new();
        let graph = KnowledgeGraph::in_memory().await.unwrap();

        let mag = graph
            .add_node(NodeData::new("magnesium", NodeType::Ingredient))
            .await;
        let mech = graph
            .add_node(NodeData::new("immune cell proliferation", NodeType::Mechanism))
            .await;
        let prop = graph
            .add_node(NodeData::new("immune defense", NodeType::Property))
            .await;

        // First link: 0.5 confidence (speculative)
        graph
            .add_edge(
                &mag,
                &mech,
                EdgeData::new(EdgeType::ViaMechanism, EdgeMetadata::emergent(0.5, 1, 0)),
            )
            .await;
        // Second link: 0.9 confidence (extracted)
        graph
            .add_edge(
                &mech,
                &prop,
                EdgeData::new(EdgeType::Affords, EdgeMetadata::extracted(0.9, 1, 0)),
            )
            .await;

        let corr_id = Uuid::new_v4();
        let result = run_forward_chaining(&sink, &graph, corr_id).await;

        assert_eq!(result.edges_added, 1);

        // Confidence should be min(0.5, 0.9) = 0.5
        let mag_edges = graph.outgoing_edges(&mag).await;
        let deduced = mag_edges
            .iter()
            .find(|(_, data)| data.metadata.source == Source::Deduced)
            .unwrap();
        assert!(
            (deduced.1.metadata.confidence - 0.5).abs() < 0.001,
            "confidence should be 0.5 (weakest link)"
        );
    }

    #[tokio::test]
    async fn test_emits_forward_chain_event() {
        let sink = MemorySink::new();
        let graph = KnowledgeGraph::in_memory().await.unwrap();

        let mag = graph
            .add_node(NodeData::new("magnesium", NodeType::Ingredient))
            .await;
        let mech = graph
            .add_node(NodeData::new("neurotransmission", NodeType::Mechanism))
            .await;
        let prop = graph
            .add_node(NodeData::new("cognitive function", NodeType::Property))
            .await;

        graph
            .add_edge(
                &mag,
                &mech,
                EdgeData::new(EdgeType::ViaMechanism, EdgeMetadata::extracted(0.7, 1, 0)),
            )
            .await;
        graph
            .add_edge(
                &mech,
                &prop,
                EdgeData::new(EdgeType::Affords, EdgeMetadata::extracted(0.8, 1, 0)),
            )
            .await;

        let corr_id = Uuid::new_v4();
        run_forward_chaining(&sink, &graph, corr_id).await;

        let events = sink.events_for(corr_id);
        let has_chain = events
            .iter()
            .any(|e| matches!(e.event, PipelineEvent::ForwardChain { .. }));
        assert!(has_chain, "should emit ForwardChain event");
    }
}
