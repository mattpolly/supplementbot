use chrono::Utc;
use event_log::events::PipelineEvent;
use event_log::sink::EventSink;
use graph_service::graph::{KnowledgeGraph, NodeIndex};
use graph_service::source::SourceStore;
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
//   2. A →[acts_on]→ S, A →[via_mechanism]→ M  ⟹  M →[modulates]→ S
//      "If ingredient A acts on system S and works through mechanism M,
//       then mechanism M modulates system S."
//      This enables the ViaMechanism query pattern:
//      Symptom →[presents_in]→ System ←[modulates]← Mechanism ←[via_mechanism]← Ingredient
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
    /// Max reasoning depth of the two premise edges
    premise_max_depth: u32,
}

/// A deduction for rule 2: Mechanism modulates System
#[derive(Debug, Clone)]
struct ModulatesDeduction {
    mechanism: NodeIndex,
    mechanism_name: String,
    system: NodeIndex,
    system_name: String,
    /// The ingredient that links them (for event logging)
    via_ingredient: String,
    confidence: f64,
    premise_max_depth: u32,
}

/// Maximum reasoning depth for premise edges. Forward chaining will not
/// operate on edges that are already the result of deep reasoning chains.
/// This prevents speculation → deduction → speculation cascades.
const MAX_PREMISE_DEPTH: u32 = 1;

/// Run forward chaining over the graph. Pure symbolic — no LLM calls.
///
/// Finds all `A → via_mechanism → M → affords → P` chains where the
/// shortcut `A → affords → P` does not yet exist, and adds it.
/// Skips premise edges with reasoning_depth > MAX_PREMISE_DEPTH.
pub async fn run_forward_chaining(
    sink: &dyn EventSink,
    graph: &KnowledgeGraph,
    source_store: Option<&SourceStore>,
    correlation_id: Uuid,
) -> ForwardChainResult {
    let mut deductions = Vec::new();

    // Find all nodes that have outgoing via_mechanism edges
    let all_nodes = graph.all_nodes().await;

    for node in &all_nodes {
        let outgoing = graph.outgoing_edges(node).await;

        // Find via_mechanism edges from this node (skip deep reasoning chains)
        let mechanisms: Vec<_> = outgoing
            .iter()
            .filter(|(_, data)| {
                data.edge_type == EdgeType::ViaMechanism
                    && data.metadata.reasoning_depth <= MAX_PREMISE_DEPTH
            })
            .collect();

        if mechanisms.is_empty() {
            continue;
        }

        // For each mechanism, check if M → affords → P exists
        for (mech_idx, mech_edge) in &mechanisms {
            let mech_outgoing = graph.outgoing_edges(mech_idx).await;

            let affordances: Vec<_> = mech_outgoing
                .iter()
                .filter(|(_, data)| {
                    data.edge_type == EdgeType::Affords
                        && data.metadata.reasoning_depth <= MAX_PREMISE_DEPTH
                })
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
                        let premise_max_depth = mech_edge
                            .metadata
                            .reasoning_depth
                            .max(prop_edge.metadata.reasoning_depth);

                        deductions.push(Deduction {
                            source: node.clone(),
                            source_name: s.name,
                            mechanism_name: m.name,
                            target: prop_idx.clone(),
                            target_name: p.name,
                            confidence,
                            premise_max_depth,
                        });
                    }
                }
            }
        }
    }

    // Rule 2: A →[acts_on]→ S, A →[via_mechanism]→ M  ⟹  M →[modulates]→ S
    let mut modulates_deductions = Vec::new();

    for node in &all_nodes {
        let outgoing = graph.outgoing_edges(node).await;

        // Collect acts_on targets (Systems) and via_mechanism targets (Mechanisms)
        let systems: Vec<_> = outgoing
            .iter()
            .filter(|(_, data)| {
                data.edge_type == EdgeType::ActsOn
                    && data.metadata.reasoning_depth <= MAX_PREMISE_DEPTH
            })
            .collect();

        let mechanisms: Vec<_> = outgoing
            .iter()
            .filter(|(_, data)| {
                data.edge_type == EdgeType::ViaMechanism
                    && data.metadata.reasoning_depth <= MAX_PREMISE_DEPTH
            })
            .collect();

        if systems.is_empty() || mechanisms.is_empty() {
            continue;
        }

        let ingredient_data = match graph.node_data(node).await {
            Some(d) if d.node_type == NodeType::Ingredient => d,
            _ => continue,
        };

        // For each (mechanism, system) pair, check if M →[modulates]→ S exists
        for (mech_idx, mech_edge) in &mechanisms {
            let mech_data = match graph.node_data(mech_idx).await {
                Some(d) if d.node_type == NodeType::Mechanism => d,
                _ => continue,
            };

            let mech_outgoing = graph.outgoing_edges(mech_idx).await;

            for (sys_idx, sys_edge) in &systems {
                let sys_data = match graph.node_data(sys_idx).await {
                    Some(d) if d.node_type == NodeType::System => d,
                    _ => continue,
                };

                // Check if M →[modulates]→ S already exists
                let already_exists = mech_outgoing.iter().any(|(tgt, data)| {
                    *tgt == *sys_idx && data.edge_type == EdgeType::Modulates
                });

                if !already_exists {
                    let confidence = mech_edge
                        .metadata
                        .confidence
                        .min(sys_edge.metadata.confidence);
                    let premise_max_depth = mech_edge
                        .metadata
                        .reasoning_depth
                        .max(sys_edge.metadata.reasoning_depth);

                    modulates_deductions.push(ModulatesDeduction {
                        mechanism: mech_idx.clone(),
                        mechanism_name: mech_data.name.clone(),
                        system: sys_idx.clone(),
                        system_name: sys_data.name.clone(),
                        via_ingredient: ingredient_data.name.clone(),
                        confidence,
                        premise_max_depth,
                    });
                }
            }
        }
    }

    // Deduplicate: same (mechanism, system) pair may be deduced via multiple ingredients
    modulates_deductions.sort_by(|a, b| {
        (&a.mechanism_name, &a.system_name).cmp(&(&b.mechanism_name, &b.system_name))
    });
    modulates_deductions.dedup_by(|a, b| {
        a.mechanism_name == b.mechanism_name && a.system_name == b.system_name
    });

    let chains_found = deductions.len() + modulates_deductions.len();
    let mut edges_added = 0;

    // Apply rule 2 deductions
    for ded in &modulates_deductions {
        let metadata =
            EdgeMetadata::deduced_with_depth(ded.confidence, 1, 0, ded.premise_max_depth);
        graph
            .add_edge(
                &ded.mechanism,
                &ded.system,
                EdgeData::new(EdgeType::Modulates, metadata),
            )
            .await;
        edges_added += 1;

        if let Some(store) = source_store {
            store
                .record_edge_created(
                    &ded.mechanism_name,
                    &ded.system_name,
                    "modulates",
                    ded.confidence,
                    "Deduced",
                    "system:forward_chain",
                    "forward_chain",
                    correlation_id,
                    Utc::now(),
                )
                .await;
        }

        sink.emit(
            correlation_id,
            PipelineEvent::ForwardChain {
                rule: "acts_on + via_mechanism => modulates".to_string(),
                premise_a: format!(
                    "{} →[acts_on]→ {}",
                    ded.via_ingredient, ded.system_name
                ),
                premise_b: format!(
                    "{} →[via_mechanism]→ {}",
                    ded.via_ingredient, ded.mechanism_name
                ),
                conclusion: format!(
                    "{} →[modulates]→ {}",
                    ded.mechanism_name, ded.system_name
                ),
                confidence: ded.confidence,
            },
        );
    }

    // Apply rule 1 deductions
    for ded in &deductions {
        // Add the deduced edge (depth = max premise depth + 1)
        let metadata =
            EdgeMetadata::deduced_with_depth(ded.confidence, 1, 0, ded.premise_max_depth);
        graph
            .add_edge(
                &ded.source,
                &ded.target,
                EdgeData::new(EdgeType::Affords, metadata),
            )
            .await;
        edges_added += 1;

        // Record provenance for the deduced edge
        if let Some(store) = source_store {
            store
                .record_edge_created(
                    &ded.source_name,
                    &ded.target_name,
                    "affords",
                    ded.confidence,
                    "Deduced",
                    "system:forward_chain",
                    "forward_chain",
                    correlation_id,
                    Utc::now(),
                )
                .await;
        }

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
        let result = run_forward_chaining(&sink, &graph, None, corr_id).await;

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
        let result = run_forward_chaining(&sink, &graph, None, corr_id).await;

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
        let result = run_forward_chaining(&sink, &graph, None, corr_id).await;

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
        run_forward_chaining(&sink, &graph, None, corr_id).await;

        let events = sink.events_for(corr_id);
        let has_chain = events
            .iter()
            .any(|e| matches!(e.event, PipelineEvent::ForwardChain { .. }));
        assert!(has_chain, "should emit ForwardChain event");
    }

    #[tokio::test]
    async fn test_deduces_modulates_from_acts_on_and_via_mechanism() {
        let sink = MemorySink::new();
        let graph = KnowledgeGraph::in_memory().await.unwrap();

        // Build: magnesium →[acts_on]→ nervous system
        //        magnesium →[via_mechanism]→ NMDA receptor modulation
        let mag = graph
            .add_node(NodeData::new("magnesium", NodeType::Ingredient))
            .await;
        let system = graph
            .add_node(NodeData::new("nervous system", NodeType::System))
            .await;
        let mech = graph
            .add_node(NodeData::new("NMDA receptor modulation", NodeType::Mechanism))
            .await;

        graph
            .add_edge(
                &mag,
                &system,
                EdgeData::new(EdgeType::ActsOn, EdgeMetadata::extracted(0.8, 1, 0)),
            )
            .await;
        graph
            .add_edge(
                &mag,
                &mech,
                EdgeData::new(EdgeType::ViaMechanism, EdgeMetadata::extracted(0.7, 1, 0)),
            )
            .await;

        let corr_id = Uuid::new_v4();
        let result = run_forward_chaining(&sink, &graph, None, corr_id).await;

        // Should deduce: NMDA receptor modulation →[modulates]→ nervous system
        assert!(result.edges_added >= 1, "should add modulates edge");

        let mech_edges = graph.outgoing_edges(&mech).await;
        let has_modulates = mech_edges.iter().any(|(tgt, data)| {
            *tgt == system
                && data.edge_type == EdgeType::Modulates
                && data.metadata.source == Source::Deduced
        });
        assert!(has_modulates, "should have deduced modulates edge");

        // Confidence should be min(0.8, 0.7) = 0.7
        let modulates_edge = mech_edges
            .iter()
            .find(|(_, data)| data.edge_type == EdgeType::Modulates)
            .unwrap();
        assert!(
            (modulates_edge.1.metadata.confidence - 0.7).abs() < 0.001,
            "confidence should be 0.7 (weakest link)"
        );
    }

    #[tokio::test]
    async fn test_modulates_not_deduced_without_via_mechanism() {
        let sink = MemorySink::new();
        let graph = KnowledgeGraph::in_memory().await.unwrap();

        // Only acts_on, no via_mechanism — should NOT deduce modulates
        let mag = graph
            .add_node(NodeData::new("magnesium", NodeType::Ingredient))
            .await;
        let system = graph
            .add_node(NodeData::new("nervous system", NodeType::System))
            .await;

        graph
            .add_edge(
                &mag,
                &system,
                EdgeData::new(EdgeType::ActsOn, EdgeMetadata::extracted(0.8, 1, 0)),
            )
            .await;

        let corr_id = Uuid::new_v4();
        let result = run_forward_chaining(&sink, &graph, None, corr_id).await;

        assert_eq!(result.edges_added, 0, "no via_mechanism means no modulates deduction");
    }
}
