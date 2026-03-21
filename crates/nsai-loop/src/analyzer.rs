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
pub fn find_gaps(graph: &KnowledgeGraph, nutraceutical: &str) -> Vec<Gap> {
    let mut gaps = Vec::new();
    let ingredient_name = nutraceutical.to_lowercase();

    for idx in graph.all_nodes() {
        let data = match graph.node_data(idx) {
            Some(d) => d,
            None => continue,
        };

        // Skip the ingredient itself — it's the root
        if data.name == ingredient_name {
            continue;
        }

        let outgoing = graph.outgoing_edges(idx);
        let incoming = graph.incoming_edges(idx);

        // Leaf node: no outgoing edges and not an Ingredient
        if outgoing.is_empty() {
            gaps.push(Gap {
                node_idx: idx,
                node_name: data.name.clone(),
                kind: GapKind::LeafNode,
            });
        }

        // Property with no incoming edge from a Mechanism node — effect without cause
        if data.node_type == NodeType::Property {
            let has_mechanism_source = incoming.iter().any(|(src_idx, _)| {
                graph
                    .node_data(*src_idx)
                    .map(|d| d.node_type == NodeType::Mechanism)
                    .unwrap_or(false)
            });
            if !has_mechanism_source {
                gaps.push(Gap {
                    node_idx: idx,
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

    #[test]
    fn test_finds_leaf_nodes() {
        let mut graph = KnowledgeGraph::new();
        let mag = graph.add_node(NodeData::new("magnesium", NodeType::Ingredient));
        let prop = graph.add_node(NodeData::new("muscle relaxation", NodeType::Property));
        graph.add_edge(
            mag,
            prop,
            EdgeData::new(EdgeType::Affords, EdgeMetadata::extracted(0.7, 1, 0)),
        );

        let gaps = find_gaps(&graph, "magnesium");

        // "muscle relaxation" is a leaf — no outgoing edges
        assert!(gaps.iter().any(|g| g.node_name == "muscle relaxation"
            && g.kind == GapKind::LeafNode));
    }

    #[test]
    fn test_finds_property_without_mechanism() {
        let mut graph = KnowledgeGraph::new();
        let mag = graph.add_node(NodeData::new("magnesium", NodeType::Ingredient));
        let prop = graph.add_node(NodeData::new("sleep quality", NodeType::Property));
        graph.add_edge(
            mag,
            prop,
            EdgeData::new(EdgeType::Affords, EdgeMetadata::extracted(0.7, 1, 0)),
        );

        let gaps = find_gaps(&graph, "magnesium");

        assert!(gaps
            .iter()
            .any(|g| g.node_name == "sleep quality" && g.kind == GapKind::NoMechanism));
    }

    #[test]
    fn test_no_gap_when_mechanism_exists() {
        let mut graph = KnowledgeGraph::new();
        let mag = graph.add_node(NodeData::new("magnesium", NodeType::Ingredient));
        let mech = graph.add_node(NodeData::new("calcium antagonism", NodeType::Mechanism));
        let prop = graph.add_node(NodeData::new("muscle relaxation", NodeType::Property));

        graph.add_edge(
            mag,
            mech,
            EdgeData::new(EdgeType::ViaMechanism, EdgeMetadata::extracted(0.7, 1, 0)),
        );
        graph.add_edge(
            mech,
            prop,
            EdgeData::new(EdgeType::Affords, EdgeMetadata::extracted(0.7, 1, 0)),
        );

        let gaps = find_gaps(&graph, "magnesium");

        // "muscle relaxation" has an incoming edge from a Mechanism node, so no NoMechanism gap
        let no_mech_gaps: Vec<_> = gaps
            .iter()
            .filter(|g| g.node_name == "muscle relaxation" && g.kind == GapKind::NoMechanism)
            .collect();
        assert!(no_mech_gaps.is_empty(), "should not flag NoMechanism when a Mechanism feeds into it");
    }

    #[test]
    fn test_skips_ingredient_node() {
        let mut graph = KnowledgeGraph::new();
        graph.add_node(NodeData::new("magnesium", NodeType::Ingredient));

        let gaps = find_gaps(&graph, "magnesium");

        // Ingredient itself should never show up as a gap
        assert!(gaps.iter().all(|g| g.node_name != "magnesium"));
    }
}
