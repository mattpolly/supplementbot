use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use petgraph::Direction;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

use crate::types::*;

// ---------------------------------------------------------------------------
// Serializable graph format — petgraph's DiGraph isn't directly serde-friendly,
// so we use an adjacency list representation for JSON roundtrips.
// ---------------------------------------------------------------------------
#[derive(Debug, Serialize, Deserialize)]
struct SerializableGraph {
    nodes: Vec<NodeData>,
    edges: Vec<SerializableEdge>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SerializableEdge {
    source: usize,
    target: usize,
    data: EdgeData,
}

// ---------------------------------------------------------------------------
// KnowledgeGraph — the petgraph wrapper
// ---------------------------------------------------------------------------
pub struct KnowledgeGraph {
    graph: DiGraph<NodeData, EdgeData>,
    /// Name → NodeIndex lookup for fast node access
    name_index: HashMap<String, NodeIndex>,
}

impl KnowledgeGraph {
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            name_index: HashMap::new(),
        }
    }

    // -- Node operations --------------------------------------------------

    /// Add a node. Returns the index. If a node with this name already exists, returns the existing index.
    pub fn add_node(&mut self, data: NodeData) -> NodeIndex {
        if let Some(&idx) = self.name_index.get(&data.name) {
            return idx;
        }
        let name = data.name.clone();
        let idx = self.graph.add_node(data);
        self.name_index.insert(name, idx);
        idx
    }

    /// Look up a node by name
    pub fn find_node(&self, name: &str) -> Option<NodeIndex> {
        self.name_index.get(name).copied()
    }

    /// Get node data by index
    pub fn node_data(&self, idx: NodeIndex) -> Option<&NodeData> {
        self.graph.node_weight(idx)
    }

    /// Get all nodes of a given type
    pub fn nodes_by_type(&self, node_type: &NodeType) -> Vec<NodeIndex> {
        self.graph
            .node_indices()
            .filter(|&idx| self.graph[idx].node_type == *node_type)
            .collect()
    }

    // -- Edge operations --------------------------------------------------

    /// Add an edge between two nodes. Does not deduplicate — caller is responsible.
    pub fn add_edge(&mut self, source: NodeIndex, target: NodeIndex, data: EdgeData) {
        self.graph.add_edge(source, target, data);
    }

    /// Get all outgoing edges from a node
    pub fn outgoing_edges(&self, idx: NodeIndex) -> Vec<(NodeIndex, &EdgeData)> {
        self.graph
            .edges_directed(idx, Direction::Outgoing)
            .map(|e| (e.target(), e.weight()))
            .collect()
    }

    /// Get all incoming edges to a node
    pub fn incoming_edges(&self, idx: NodeIndex) -> Vec<(NodeIndex, &EdgeData)> {
        self.graph
            .edges_directed(idx, Direction::Incoming)
            .map(|e| (e.source(), e.weight()))
            .collect()
    }

    /// Find all neighbors of a node (outgoing direction) filtered by edge type
    pub fn neighbors_by_edge_type(
        &self,
        idx: NodeIndex,
        edge_type: &EdgeType,
    ) -> Vec<(NodeIndex, &EdgeData)> {
        self.outgoing_edges(idx)
            .into_iter()
            .filter(|(_, data)| data.edge_type == *edge_type)
            .collect()
    }

    // -- Graph stats ------------------------------------------------------

    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    // -- Serialization ----------------------------------------------------

    /// Serialize the full graph to a JSON string
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        let sg = self.to_serializable();
        serde_json::to_string_pretty(&sg)
    }

    /// Deserialize a graph from a JSON string
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let sg: SerializableGraph = serde_json::from_str(json)?;
        Ok(Self::from_serializable(sg))
    }

    fn to_serializable(&self) -> SerializableGraph {
        let nodes: Vec<NodeData> = self
            .graph
            .node_indices()
            .map(|idx| self.graph[idx].clone())
            .collect();

        // Build a NodeIndex → usize map for edge serialization
        let index_map: HashMap<NodeIndex, usize> = self
            .graph
            .node_indices()
            .enumerate()
            .map(|(i, idx)| (idx, i))
            .collect();

        let edges: Vec<SerializableEdge> = self
            .graph
            .edge_indices()
            .filter_map(|eidx| {
                let (src, tgt) = self.graph.edge_endpoints(eidx)?;
                let data = self.graph[eidx].clone();
                Some(SerializableEdge {
                    source: index_map[&src],
                    target: index_map[&tgt],
                    data,
                })
            })
            .collect();

        SerializableGraph { nodes, edges }
    }

    fn from_serializable(sg: SerializableGraph) -> Self {
        let mut kg = KnowledgeGraph::new();
        let mut index_map: Vec<NodeIndex> = Vec::with_capacity(sg.nodes.len());

        for node in sg.nodes {
            let idx = kg.add_node(node);
            index_map.push(idx);
        }

        for edge in sg.edges {
            kg.add_edge(index_map[edge.source], index_map[edge.target], edge.data);
        }

        kg
    }
}

impl Default for KnowledgeGraph {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Display — human-readable graph dump
// "Magnesium (Ingredient) →[acts_on, confidence: 0.92, Extracted, epoch: 0]→ Nervous System (System)"
// ---------------------------------------------------------------------------
impl fmt::Display for KnowledgeGraph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "KnowledgeGraph ({} nodes, {} edges)",
            self.node_count(),
            self.edge_count()
        )?;
        writeln!(f, "{}", "-".repeat(60))?;

        for eidx in self.graph.edge_indices() {
            if let Some((src, tgt)) = self.graph.edge_endpoints(eidx) {
                let src_data = &self.graph[src];
                let tgt_data = &self.graph[tgt];
                let edge_data = &self.graph[eidx];
                writeln!(f, "  {} →[{}]→ {}", src_data, edge_data, tgt_data)?;
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a small graph with Magnesium acting on Nervous System via NMDA antagonism
    fn build_magnesium_graph() -> KnowledgeGraph {
        let mut kg = KnowledgeGraph::new();

        let mag = kg.add_node(NodeData::new("Magnesium", NodeType::Ingredient));
        let nervous = kg.add_node(NodeData::new("Nervous System", NodeType::System));
        let nmda = kg.add_node(NodeData::new("NMDA Receptor Antagonism", NodeType::Mechanism));

        kg.add_edge(
            mag,
            nervous,
            EdgeData::new(EdgeType::ActsOn, EdgeMetadata::extracted(0.92, 1, 0)),
        );
        kg.add_edge(
            mag,
            nmda,
            EdgeData::new(EdgeType::ViaMechanism, EdgeMetadata::extracted(0.88, 1, 0)),
        );
        kg.add_edge(
            nmda,
            nervous,
            EdgeData::new(EdgeType::Modulates, EdgeMetadata::extracted(0.85, 1, 0)),
        );

        kg
    }

    #[test]
    fn test_node_creation_and_lookup() {
        let kg = build_magnesium_graph();
        assert_eq!(kg.node_count(), 3);
        assert_eq!(kg.edge_count(), 3);

        let mag_idx = kg.find_node("Magnesium").unwrap();
        let mag_data = kg.node_data(mag_idx).unwrap();
        assert_eq!(mag_data.name, "Magnesium");
        assert_eq!(mag_data.node_type, NodeType::Ingredient);
    }

    #[test]
    fn test_duplicate_node_returns_existing() {
        let mut kg = KnowledgeGraph::new();
        let idx1 = kg.add_node(NodeData::new("Magnesium", NodeType::Ingredient));
        let idx2 = kg.add_node(NodeData::new("Magnesium", NodeType::Ingredient));
        assert_eq!(idx1, idx2);
        assert_eq!(kg.node_count(), 1);
    }

    #[test]
    fn test_neighbors_by_edge_type() {
        let kg = build_magnesium_graph();
        let mag_idx = kg.find_node("Magnesium").unwrap();

        let acts_on = kg.neighbors_by_edge_type(mag_idx, &EdgeType::ActsOn);
        assert_eq!(acts_on.len(), 1);

        let target_data = kg.node_data(acts_on[0].0).unwrap();
        assert_eq!(target_data.name, "Nervous System");
    }

    #[test]
    fn test_nodes_by_type() {
        let kg = build_magnesium_graph();
        let systems = kg.nodes_by_type(&NodeType::System);
        assert_eq!(systems.len(), 1);

        let mechanisms = kg.nodes_by_type(&NodeType::Mechanism);
        assert_eq!(mechanisms.len(), 1);
    }

    #[test]
    fn test_cross_system_traversal() {
        let mut kg = build_magnesium_graph();

        // Add GI system — magnesium also acts on it (broad-spectrum)
        let gi = kg.add_node(NodeData::new("Gastrointestinal System", NodeType::System));
        let smooth_muscle =
            kg.add_node(NodeData::new("Smooth Muscle Relaxation", NodeType::Mechanism));
        let mag_idx = kg.find_node("Magnesium").unwrap();

        kg.add_edge(
            mag_idx,
            gi,
            EdgeData::new(EdgeType::ActsOn, EdgeMetadata::extracted(0.87, 1, 0)),
        );
        kg.add_edge(
            mag_idx,
            smooth_muscle,
            EdgeData::new(EdgeType::ViaMechanism, EdgeMetadata::extracted(0.80, 1, 0)),
        );
        kg.add_edge(
            smooth_muscle,
            gi,
            EdgeData::new(EdgeType::Modulates, EdgeMetadata::extracted(0.78, 1, 0)),
        );

        // Magnesium should now act on two systems
        let acts_on = kg.neighbors_by_edge_type(mag_idx, &EdgeType::ActsOn);
        assert_eq!(acts_on.len(), 2);

        // Both systems should be reachable
        let system_names: Vec<&str> = acts_on
            .iter()
            .map(|(idx, _)| kg.node_data(*idx).unwrap().name.as_str())
            .collect();
        assert!(system_names.contains(&"Nervous System"));
        assert!(system_names.contains(&"Gastrointestinal System"));
    }

    #[test]
    fn test_json_roundtrip() {
        let kg = build_magnesium_graph();
        let json = kg.to_json().expect("serialization failed");
        let kg2 = KnowledgeGraph::from_json(&json).expect("deserialization failed");

        assert_eq!(kg2.node_count(), kg.node_count());
        assert_eq!(kg2.edge_count(), kg.edge_count());

        // Verify data survived the roundtrip
        let mag_idx = kg2.find_node("Magnesium").unwrap();
        let mag_data = kg2.node_data(mag_idx).unwrap();
        assert_eq!(mag_data.node_type, NodeType::Ingredient);

        let acts_on = kg2.neighbors_by_edge_type(mag_idx, &EdgeType::ActsOn);
        assert_eq!(acts_on.len(), 1);
        assert!((acts_on[0].1.metadata.confidence - 0.92).abs() < f64::EPSILON);
    }

    #[test]
    fn test_extra_metadata_roundtrip() {
        let mut kg = KnowledgeGraph::new();
        let mag = kg.add_node(NodeData::new("Magnesium", NodeType::Ingredient));
        let nervous = kg.add_node(NodeData::new("Nervous System", NodeType::System));

        let mut meta = EdgeMetadata::extracted(0.9, 1, 0);
        meta.extra
            .insert("dosage_dependent".into(), MetadataValue::Bool(true));
        meta.extra.insert(
            "min_effective_mg".into(),
            MetadataValue::Float(200.0),
        );
        meta.extra.insert(
            "note".into(),
            MetadataValue::String("effect significant above 200mg".into()),
        );

        kg.add_edge(mag, nervous, EdgeData::new(EdgeType::ActsOn, meta));

        // Roundtrip through JSON
        let json = kg.to_json().unwrap();
        let kg2 = KnowledgeGraph::from_json(&json).unwrap();

        let mag_idx = kg2.find_node("Magnesium").unwrap();
        let edges = kg2.outgoing_edges(mag_idx);
        let extra = &edges[0].1.metadata.extra;

        assert_eq!(extra.get("dosage_dependent"), Some(&MetadataValue::Bool(true)));
        assert_eq!(extra.get("min_effective_mg"), Some(&MetadataValue::Float(200.0)));
    }

    #[test]
    fn test_display() {
        let kg = build_magnesium_graph();
        let output = format!("{}", kg);
        assert!(output.contains("Magnesium (Ingredient)"));
        assert!(output.contains("acts_on"));
        assert!(output.contains("Nervous System (System)"));
        assert!(output.contains("confidence: 0.92"));
    }
}
