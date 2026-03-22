use graph_service::graph::{KnowledgeGraph, NodeIndex};
use graph_service::types::*;

// ---------------------------------------------------------------------------
// Gap types — what the analyzer can identify
// ---------------------------------------------------------------------------

/// A gap in the knowledge graph that could be filled
#[derive(Debug, Clone)]
pub struct Gap {
    /// The node where the gap was found
    pub node_idx: NodeIndex,
    /// Human-readable name of the node
    pub node_name: String,
    /// What kind of gap this is
    pub kind: GapKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GapKind {
    /// Node has no outgoing edges — it's a dead end (e.g. "muscle relaxation" with no explanation of how)
    LeafNode,
    /// Property or Mechanism node with no via_mechanism edge pointing to it — effect without cause
    NoMechanism,
    /// System node that the ingredient has no acts_on edge to, but is reachable through a mechanism
    IndirectSystem,
}

impl GapKind {
    pub fn label(&self) -> &'static str {
        match self {
            GapKind::LeafNode => "leaf_node",
            GapKind::NoMechanism => "no_mechanism",
            GapKind::IndirectSystem => "indirect_system",
        }
    }

    pub fn description(&self, node_name: &str) -> String {
        match self {
            GapKind::LeafNode => {
                format!("\"{}\" has no outgoing edges — it's unexplained", node_name)
            }
            GapKind::NoMechanism => {
                format!("\"{}\" has no mechanism explaining how it works", node_name)
            }
            GapKind::IndirectSystem => {
                format!(
                    "\"{}\" is only reachable indirectly — no direct ingredient connection",
                    node_name
                )
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Analyze
// ---------------------------------------------------------------------------

/// Analyze the graph and return gaps worth filling.
///
/// Skips the ingredient node itself (it's the root, not a gap).
pub async fn find_gaps(graph: &KnowledgeGraph, nutraceutical: &str) -> Vec<Gap> {
    let mut gaps = Vec::new();
    let ingredient_name = nutraceutical.to_lowercase();

    for idx in graph.all_nodes().await {
        let data = match graph.node_data(&idx).await {
            Some(d) => d,
            None => continue,
        };

        // Skip the ingredient itself — it's the root
        if data.name == ingredient_name {
            continue;
        }

        let outgoing = graph.outgoing_edges(&idx).await;
        let incoming = graph.incoming_edges(&idx).await;

        // Leaf node: no outgoing edges and not an Ingredient.
        // But System and Property nodes are valid terminals — they're
        // targets, not sources. Only flag them if they have zero
        // incoming edges (truly disconnected).
        if outgoing.is_empty() {
            let is_terminal = match data.node_type {
                NodeType::System => incoming.iter().any(|(_, e)| e.edge_type == EdgeType::ActsOn),
                NodeType::Property => incoming.iter().any(|(_, e)| e.edge_type == EdgeType::Affords),
                _ => false,
            };

            if !is_terminal {
                gaps.push(Gap {
                    node_idx: idx.clone(),
                    node_name: data.name.clone(),
                    kind: GapKind::LeafNode,
                });
            }
        }

        // Property with no incoming edge from a Mechanism node — effect without cause
        if data.node_type == NodeType::Property {
            let mut has_mechanism_source = false;
            for (src_idx, _) in &incoming {
                if let Some(d) = graph.node_data(src_idx).await {
                    if d.node_type == NodeType::Mechanism {
                        has_mechanism_source = true;
                        break;
                    }
                }
            }
            if !has_mechanism_source {
                gaps.push(Gap {
                    node_idx: idx.clone(),
                    node_name: data.name.clone(),
                    kind: GapKind::NoMechanism,
                });
            }
        }
    }

    gaps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_finds_leaf_nodes() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let mag = graph.add_node(NodeData::new("magnesium", NodeType::Ingredient)).await;
        // Mechanism with no outgoing edges and no terminal-qualifying incoming edges
        let mech = graph.add_node(NodeData::new("calcium antagonism", NodeType::Mechanism)).await;
        graph.add_edge(
            &mag,
            &mech,
            EdgeData::new(EdgeType::ViaMechanism, EdgeMetadata::extracted(0.7, 1, 0)),
        ).await;

        let gaps = find_gaps(&graph, "magnesium").await;

        assert!(gaps.iter().any(|g| g.node_name == "calcium antagonism"
            && g.kind == GapKind::LeafNode));
    }

    #[tokio::test]
    async fn test_system_with_acts_on_is_terminal() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let mag = graph.add_node(NodeData::new("magnesium", NodeType::Ingredient)).await;
        let sys = graph.add_node(NodeData::new("muscular system", NodeType::System)).await;
        graph.add_edge(
            &mag,
            &sys,
            EdgeData::new(EdgeType::ActsOn, EdgeMetadata::extracted(0.7, 1, 0)),
        ).await;

        let gaps = find_gaps(&graph, "magnesium").await;

        // System with incoming acts_on is a valid terminal — not a gap
        assert!(
            !gaps.iter().any(|g| g.node_name == "muscular system" && g.kind == GapKind::LeafNode),
            "System with acts_on should not be flagged as leaf"
        );
    }

    #[tokio::test]
    async fn test_property_with_affords_is_terminal() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let mag = graph.add_node(NodeData::new("magnesium", NodeType::Ingredient)).await;
        let prop = graph.add_node(NodeData::new("muscle relaxation", NodeType::Property)).await;
        graph.add_edge(
            &mag,
            &prop,
            EdgeData::new(EdgeType::Affords, EdgeMetadata::extracted(0.7, 1, 0)),
        ).await;

        let gaps = find_gaps(&graph, "magnesium").await;

        // Property with incoming affords is a valid terminal — not a leaf gap
        assert!(
            !gaps.iter().any(|g| g.node_name == "muscle relaxation" && g.kind == GapKind::LeafNode),
            "Property with affords should not be flagged as leaf"
        );
    }

    #[tokio::test]
    async fn test_disconnected_system_is_still_a_gap() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        graph.add_node(NodeData::new("magnesium", NodeType::Ingredient)).await;
        // System with zero incoming edges — genuinely disconnected
        graph.add_node(NodeData::new("skeletal system", NodeType::System)).await;

        let gaps = find_gaps(&graph, "magnesium").await;

        assert!(
            gaps.iter().any(|g| g.node_name == "skeletal system" && g.kind == GapKind::LeafNode),
            "Disconnected system should still be flagged"
        );
    }

    #[tokio::test]
    async fn test_finds_property_without_mechanism() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let mag = graph.add_node(NodeData::new("magnesium", NodeType::Ingredient)).await;
        let prop = graph.add_node(NodeData::new("sleep quality", NodeType::Property)).await;
        graph.add_edge(
            &mag,
            &prop,
            EdgeData::new(EdgeType::Affords, EdgeMetadata::extracted(0.7, 1, 0)),
        ).await;

        let gaps = find_gaps(&graph, "magnesium").await;

        assert!(gaps
            .iter()
            .any(|g| g.node_name == "sleep quality" && g.kind == GapKind::NoMechanism));
    }

    #[tokio::test]
    async fn test_no_gap_when_mechanism_exists() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let mag = graph.add_node(NodeData::new("magnesium", NodeType::Ingredient)).await;
        let mech = graph.add_node(NodeData::new("calcium antagonism", NodeType::Mechanism)).await;
        let prop = graph.add_node(NodeData::new("muscle relaxation", NodeType::Property)).await;

        graph.add_edge(
            &mag,
            &mech,
            EdgeData::new(EdgeType::ViaMechanism, EdgeMetadata::extracted(0.7, 1, 0)),
        ).await;
        graph.add_edge(
            &mech,
            &prop,
            EdgeData::new(EdgeType::Affords, EdgeMetadata::extracted(0.7, 1, 0)),
        ).await;

        let gaps = find_gaps(&graph, "magnesium").await;

        // "muscle relaxation" has an incoming edge from a Mechanism node, so no NoMechanism gap
        let no_mech_gaps: Vec<_> = gaps
            .iter()
            .filter(|g| g.node_name == "muscle relaxation" && g.kind == GapKind::NoMechanism)
            .collect();
        assert!(no_mech_gaps.is_empty(), "should not flag NoMechanism when a Mechanism feeds into it");
    }

    #[tokio::test]
    async fn test_skips_ingredient_node() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        graph.add_node(NodeData::new("magnesium", NodeType::Ingredient)).await;

        let gaps = find_gaps(&graph, "magnesium").await;

        // Ingredient itself should never show up as a gap
        assert!(gaps.iter().all(|g| g.node_name != "magnesium"));
    }
}
