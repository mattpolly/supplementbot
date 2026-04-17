use surrealdb::engine::local::{Db, Mem, RocksDb};
use surrealdb::Surreal;
use surrealdb_types::{RecordId, SurrealValue};

use crate::export::{ExportEdge, ExportGraph, ExportNode};
use crate::types::*;

/// Extract the string key from a RecordId (our keys are always slugified strings).
fn record_key(id: &RecordId) -> String {
    match &id.key {
        surrealdb_types::RecordIdKey::String(s) => s.clone(),
        other => format!("{:?}", other),
    }
}

// ---------------------------------------------------------------------------
// NodeIndex — a lightweight handle to a node in the database
// ---------------------------------------------------------------------------

/// Opaque handle to a graph node. Wraps a SurrealDB RecordId.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeIndex(RecordId);

impl NodeIndex {
    pub fn id(&self) -> &RecordId {
        &self.0
    }

    /// Create a dummy NodeIndex for use in tests that don't need a real DB connection.
    pub fn default_for_test() -> Self {
        Self(RecordId::new("node", "test_dummy"))
    }
}

// ---------------------------------------------------------------------------
// DB record types — what SurrealDB stores
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, SurrealValue)]
struct NodeRecord {
    name: String,
    node_type: NodeType,
}

/// A node record as returned from SurrealDB SELECT (includes `id` field)
#[derive(Debug, Clone, SurrealValue)]
struct NodeRecordWithId {
    id: RecordId,
    name: String,
    node_type: NodeType,
}

#[derive(Debug, Clone, SurrealValue)]
struct EdgeRecordWithId {
    id: RecordId,
    source: RecordId,
    target: RecordId,
    edge_type: EdgeType,
    metadata: EdgeMetadata,
}

// ---------------------------------------------------------------------------
// KnowledgeGraph — SurrealDB-backed graph
// ---------------------------------------------------------------------------

pub struct KnowledgeGraph {
    db: Surreal<Db>,
}

impl KnowledgeGraph {
    /// Open or create a persistent graph database at the given path.
    pub async fn open(path: &str) -> Result<Self, surrealdb::Error> {
        let db = Surreal::new::<RocksDb>(path).await?;
        db.use_ns("supplementbot").use_db("graph").await?;
        // Ensure the edge table is typed as a relation so that `in`/`out`
        // are hydrated during full table scans (SurrealDB 3.0 requirement).
        let _: surrealdb::Result<Vec<serde_json::Value>> = db
            .query("DEFINE TABLE edge TYPE RELATION IN node OUT node")
            .await
            .and_then(|mut r| r.take(0));
        Ok(Self { db })
    }

    /// Create an in-memory graph (for tests).
    pub async fn in_memory() -> Result<Self, surrealdb::Error> {
        let db = Surreal::new::<Mem>(()).await?;
        db.use_ns("supplementbot").use_db("graph").await?;
        Ok(Self { db })
    }

    /// Get a reference to the underlying SurrealDB handle.
    /// Used by the evidence layer to share the same database connection.
    pub fn db(&self) -> &Surreal<Db> {
        &self.db
    }

    // -- Node operations --------------------------------------------------

    /// Add a node. If a node with this name already exists, returns the existing index.
    /// Deduplicates by lowercase name.
    pub async fn add_node(&self, data: NodeData) -> NodeIndex {
        // Check if node already exists by name
        if let Some(idx) = self.find_node(&data.name).await {
            return idx;
        }

        // Use the lowercase name as the record ID for natural dedup
        let key = slug(&data.name);
        let record: Option<NodeRecordWithId> = self
            .db
            .create(("node", key.as_str()))
            .content(NodeRecord {
                name: data.name,
                node_type: data.node_type,
            })
            .await
            .ok()
            .flatten();

        match record {
            Some(r) => NodeIndex(r.id),
            None => {
                // Race condition or already exists — fetch it
                let existing: Option<NodeRecordWithId> =
                    self.db.select(("node", key.as_str())).await.ok().flatten();
                NodeIndex(existing.unwrap().id)
            }
        }
    }

    /// Look up a node by name (case-insensitive)
    pub async fn find_node(&self, name: &str) -> Option<NodeIndex> {
        let key = slug(name);
        let record: Option<NodeRecordWithId> =
            self.db.select(("node", key.as_str())).await.ok().flatten();
        record.map(|r| NodeIndex(r.id))
    }

    /// Get node data by index
    pub async fn node_data(&self, idx: &NodeIndex) -> Option<NodeData> {
        let record: Option<NodeRecordWithId> =
            self.db.select(idx.0.clone()).await.ok().flatten();
        record.map(|r| NodeData::new(r.name, r.node_type))
    }

    /// Get all nodes of a given type
    pub async fn nodes_by_type(&self, node_type: &NodeType) -> Vec<NodeIndex> {
        let mut result = self
            .db
            .query("SELECT * FROM node WHERE node_type = $nt")
            .bind(("nt", node_type.clone()))
            .await
            .unwrap();
        let records: Vec<NodeRecordWithId> = result.take(0).unwrap_or_default();
        records.into_iter().map(|r| NodeIndex(r.id)).collect()
    }

    /// Return all ingredient names the graph has been trained on, sorted alphabetically.
    pub async fn known_ingredients(&self) -> Vec<String> {
        let mut result = self
            .db
            .query("SELECT name FROM node WHERE node_type = $nt ORDER BY name ASC")
            .bind(("nt", NodeType::Ingredient))
            .await
            .unwrap();
        let records: Vec<serde_json::Value> = result.take(0).unwrap_or_default();
        records
            .into_iter()
            .filter_map(|v| v.get("name").and_then(|n| n.as_str()).map(|s| {
                // Title-case the name for display (e.g. "magnesium" → "Magnesium")
                let mut c = s.chars();
                match c.next() {
                    None => String::new(),
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                }
            }))
            .collect()
    }

    /// Find a node by name, falling back to alias resolution if not found directly.
    pub async fn find_node_or_alias(
        &self,
        name: &str,
        merge: &crate::merge::MergeStore,
    ) -> Option<NodeIndex> {
        // Try direct lookup first
        if let Some(idx) = self.find_node(name).await {
            return Some(idx);
        }
        // Resolve through aliases and try the canonical name
        let canonical = merge.resolve(name).await;
        if canonical != name.to_lowercase() {
            self.find_node(&canonical).await
        } else {
            None
        }
    }

    // -- Edge operations --------------------------------------------------

    /// Add an edge between two nodes. Does not deduplicate — caller is responsible.
    pub async fn add_edge(&self, source: &NodeIndex, target: &NodeIndex, data: EdgeData) {
        let _: surrealdb::Result<Vec<EdgeRecordWithId>> = self
            .db
            .query("RELATE $from->edge->$to SET edge_type = $et, metadata = $meta")
            .bind(("from", source.0.clone()))
            .bind(("to", target.0.clone()))
            .bind(("et", data.edge_type))
            .bind(("meta", data.metadata))
            .await
            .and_then(|mut r| r.take(0));
    }

    /// Get all outgoing edges from a node
    pub async fn outgoing_edges(&self, idx: &NodeIndex) -> Vec<(NodeIndex, EdgeData)> {
        let mut result = self
            .db
            .query("SELECT *, in AS source, out AS target FROM edge WHERE in = $node")
            .bind(("node", idx.0.clone()))
            .await
            .unwrap();

        let records: Vec<EdgeRecordWithId> = result.take(0).unwrap_or_default();
        records
            .into_iter()
            .map(|r| {
                let edge_data = EdgeData::new(r.edge_type, r.metadata);
                (NodeIndex(r.target), edge_data)
            })
            .collect()
    }

    /// Get all incoming edges to a node
    pub async fn incoming_edges(&self, idx: &NodeIndex) -> Vec<(NodeIndex, EdgeData)> {
        let mut result = self
            .db
            .query("SELECT *, in AS source, out AS target FROM edge WHERE out = $node")
            .bind(("node", idx.0.clone()))
            .await
            .unwrap();

        let records: Vec<EdgeRecordWithId> = result.take(0).unwrap_or_default();
        records
            .into_iter()
            .map(|r| {
                let edge_data = EdgeData::new(r.edge_type, r.metadata);
                (NodeIndex(r.source), edge_data)
            })
            .collect()
    }

    /// Total degree (incoming + outgoing) for a node.
    pub async fn node_degree(&self, idx: &NodeIndex) -> usize {
        let mut result = self
            .db
            .query("SELECT count() FROM edge WHERE in = $node OR out = $node GROUP ALL")
            .bind(("node", idx.0.clone()))
            .await
            .unwrap();
        let count: Option<CountResult> = result.take(0).unwrap_or_default();
        count.map(|c| c.count).unwrap_or(0)
    }

    /// Update the confidence of all edges matching (source, target, edge_type).
    /// Adds `boost` to the existing confidence, capped at 1.0.
    /// Returns the number of edges updated.
    pub async fn boost_edge_confidence(
        &self,
        source: &NodeIndex,
        target: &NodeIndex,
        edge_type: &EdgeType,
        boost: f64,
    ) -> usize {
        // SurrealDB graph relations: `in` = source, `out` = target
        // We need to find matching edges, update them, and count
        let edges = self.outgoing_edges(source).await;
        let mut updated = 0;

        for (tgt, data) in &edges {
            if *tgt == *target && data.edge_type == *edge_type {
                let new_confidence = (data.metadata.confidence + boost).min(1.0);
                let _: surrealdb::Result<Vec<EdgeRecordWithId>> = self
                    .db
                    .query(
                        "UPDATE edge SET metadata.confidence = $conf \
                         WHERE in = $from AND out = $to AND edge_type = $et",
                    )
                    .bind(("conf", new_confidence))
                    .bind(("from", source.0.clone()))
                    .bind(("to", target.0.clone()))
                    .bind(("et", edge_type.clone()))
                    .await
                    .and_then(|mut r| r.take(0));
                updated += 1;
            }
        }

        updated
    }

    /// Iterate over all node indices
    pub async fn all_nodes(&self) -> Vec<NodeIndex> {
        let records: Vec<NodeRecordWithId> =
            self.db.select("node").await.unwrap_or_default();
        records.into_iter().map(|r| NodeIndex(r.id)).collect()
    }

    /// Get all edges in the graph as (source, target, edge_data) triples.
    pub async fn all_edges(&self) -> Vec<(NodeIndex, NodeIndex, EdgeData)> {
        let mut result = self
            .db
            .query("SELECT *, in AS source, out AS target FROM edge")
            .await
            .unwrap();

        let records: Vec<EdgeRecordWithId> = result.take(0).unwrap_or_default();
        records
            .into_iter()
            .map(|r| {
                let edge_data = EdgeData::new(r.edge_type, r.metadata);
                (NodeIndex(r.source), NodeIndex(r.target), edge_data)
            })
            .collect()
    }

    /// Set the confidence of all edges matching (source, target, edge_type)
    /// to an exact value. Returns the number of edges updated.
    pub async fn set_edge_confidence(
        &self,
        source: &NodeIndex,
        target: &NodeIndex,
        edge_type: &EdgeType,
        confidence: f64,
    ) -> usize {
        let _: surrealdb::Result<Vec<EdgeRecordWithId>> = self
            .db
            .query(
                "UPDATE edge SET metadata.confidence = $conf \
                 WHERE in = $from AND out = $to AND edge_type = $et",
            )
            .bind(("conf", confidence.clamp(0.0, 1.0)))
            .bind(("from", source.0.clone()))
            .bind(("to", target.0.clone()))
            .bind(("et", edge_type.clone()))
            .await
            .and_then(|mut r| r.take(0));
        // We don't get a reliable count from SurrealDB UPDATE on relations,
        // so check by re-reading
        let edges = self.outgoing_edges(source).await;
        edges
            .iter()
            .filter(|(t, d)| *t == *target && d.edge_type == *edge_type)
            .count()
    }

    // -- Graph stats ------------------------------------------------------

    pub async fn node_count(&self) -> usize {
        let mut result = self
            .db
            .query("SELECT count() FROM node GROUP ALL")
            .await
            .unwrap();
        let counts: Vec<CountResult> = result.take(0).unwrap_or_default();
        counts.into_iter().next().map(|c| c.count).unwrap_or(0)
    }

    pub async fn edge_count(&self) -> usize {
        let mut result = self
            .db
            .query("SELECT count() FROM edge GROUP ALL")
            .await
            .unwrap();
        let counts: Vec<CountResult> = result.take(0).unwrap_or_default();
        counts.into_iter().next().map(|c| c.count).unwrap_or(0)
    }

    /// Export the full graph as a JSON-serializable structure for visualization.
    pub async fn export_json(&self) -> ExportGraph {
        let mut result = self
            .db
            .query("SELECT * FROM node")
            .await
            .unwrap();
        let node_records: Vec<NodeRecordWithId> = result.take(0).unwrap_or_default();

        let nodes: Vec<ExportNode> = node_records
            .iter()
            .map(|n| {
                let id = record_key(&n.id);
                ExportNode {
                    id,
                    name: n.name.clone(),
                    node_type: format!("{:?}", n.node_type),
                }
            })
            .collect();

        let mut result = self
            .db
            .query("SELECT *, in AS source, out AS target FROM edge")
            .await
            .unwrap();
        let edge_records: Vec<EdgeRecordWithId> = result.take(0).unwrap_or_default();

        let edges: Vec<ExportEdge> = edge_records
            .iter()
            .map(|e| ExportEdge {
                source: record_key(&e.source),
                target: record_key(&e.target),
                edge_type: e.edge_type.to_string(),
                confidence: e.metadata.confidence,
                source_tag: format!("{:?}", e.metadata.source),
            })
            .collect();

        ExportGraph { nodes, edges }
    }

    pub async fn dump(&self) -> String {
        let node_count = self.node_count().await;
        let edge_count = self.edge_count().await;

        let mut out = format!(
            "KnowledgeGraph ({} nodes, {} edges)\n{}\n",
            node_count,
            edge_count,
            "-".repeat(60)
        );

        let mut result = self
            .db
            .query("SELECT *, in AS source, out AS target FROM edge")
            .await
            .unwrap();
        let edges: Vec<EdgeRecordWithId> = result.take(0).unwrap_or_default();

        for edge in edges {
            let src = self.node_data(&NodeIndex(edge.source.clone())).await;
            let tgt = self.node_data(&NodeIndex(edge.target.clone())).await;
            if let (Some(s), Some(t)) = (src, tgt) {
                let edge_data = EdgeData::new(edge.edge_type, edge.metadata);
                out.push_str(&format!("  {} →[{}]→ {}\n", s, edge_data, t));
            }
        }

        out
    }
}

#[derive(Debug, SurrealValue)]
struct CountResult {
    count: usize,
}

/// Convert a node name to a SurrealDB-safe record key.
/// Lowercase, replace spaces with underscores, strip non-alphanumeric.
fn slug(name: &str) -> String {
    name.trim()
        .to_lowercase()
        .replace(' ', "_")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    async fn build_magnesium_graph() -> KnowledgeGraph {
        let kg = KnowledgeGraph::in_memory().await.unwrap();

        let mag = kg.add_node(NodeData::new("Magnesium", NodeType::Ingredient)).await;
        let nervous = kg.add_node(NodeData::new("Nervous System", NodeType::System)).await;
        let nmda = kg
            .add_node(NodeData::new("NMDA Receptor Antagonism", NodeType::Mechanism))
            .await;

        kg.add_edge(
            &mag,
            &nervous,
            EdgeData::new(EdgeType::ActsOn, EdgeMetadata::extracted(0.92, 1, 0)),
        )
        .await;
        kg.add_edge(
            &mag,
            &nmda,
            EdgeData::new(EdgeType::ViaMechanism, EdgeMetadata::extracted(0.88, 1, 0)),
        )
        .await;
        kg.add_edge(
            &nmda,
            &nervous,
            EdgeData::new(EdgeType::Modulates, EdgeMetadata::extracted(0.85, 1, 0)),
        )
        .await;

        kg
    }

    #[tokio::test]
    async fn test_node_creation_and_lookup() {
        let kg = build_magnesium_graph().await;
        assert_eq!(kg.node_count().await, 3);
        assert_eq!(kg.edge_count().await, 3);

        let mag_idx = kg.find_node("Magnesium").await.unwrap();
        let mag_data = kg.node_data(&mag_idx).await.unwrap();
        assert_eq!(mag_data.name, "Magnesium");
        assert_eq!(mag_data.node_type, NodeType::Ingredient);
    }

    #[tokio::test]
    async fn test_duplicate_node_returns_existing() {
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        let idx1 = kg.add_node(NodeData::new("Magnesium", NodeType::Ingredient)).await;
        let idx2 = kg.add_node(NodeData::new("Magnesium", NodeType::Ingredient)).await;
        assert_eq!(idx1, idx2);
        assert_eq!(kg.node_count().await, 1);
    }

    #[tokio::test]
    async fn test_outgoing_edges() {
        let kg = build_magnesium_graph().await;
        let mag_idx = kg.find_node("Magnesium").await.unwrap();

        let edges = kg.outgoing_edges(&mag_idx).await;
        let acts_on: Vec<_> = edges
            .iter()
            .filter(|(_, data)| data.edge_type == EdgeType::ActsOn)
            .collect();
        assert_eq!(acts_on.len(), 1);

        let target_data = kg.node_data(&acts_on[0].0).await.unwrap();
        assert_eq!(target_data.name, "Nervous System");
    }

    #[tokio::test]
    async fn test_nodes_by_type() {
        let kg = build_magnesium_graph().await;
        let systems = kg.nodes_by_type(&NodeType::System).await;
        assert_eq!(systems.len(), 1);

        let mechanisms = kg.nodes_by_type(&NodeType::Mechanism).await;
        assert_eq!(mechanisms.len(), 1);
    }

    #[tokio::test]
    async fn test_cross_system_traversal() {
        let kg = build_magnesium_graph().await;

        let gi = kg
            .add_node(NodeData::new("Gastrointestinal System", NodeType::System))
            .await;
        let smooth_muscle = kg
            .add_node(NodeData::new("Smooth Muscle Relaxation", NodeType::Mechanism))
            .await;
        let mag_idx = kg.find_node("Magnesium").await.unwrap();

        kg.add_edge(
            &mag_idx,
            &gi,
            EdgeData::new(EdgeType::ActsOn, EdgeMetadata::extracted(0.87, 1, 0)),
        )
        .await;
        kg.add_edge(
            &mag_idx,
            &smooth_muscle,
            EdgeData::new(EdgeType::ViaMechanism, EdgeMetadata::extracted(0.80, 1, 0)),
        )
        .await;
        kg.add_edge(
            &smooth_muscle,
            &gi,
            EdgeData::new(EdgeType::Modulates, EdgeMetadata::extracted(0.78, 1, 0)),
        )
        .await;

        let edges = kg.outgoing_edges(&mag_idx).await;
        let acts_on: Vec<_> = edges
            .iter()
            .filter(|(_, data)| data.edge_type == EdgeType::ActsOn)
            .collect();
        assert_eq!(acts_on.len(), 2);

        let mut system_names: Vec<String> = Vec::new();
        for (idx, _) in &acts_on {
            let data = kg.node_data(idx).await.unwrap();
            system_names.push(data.name.clone());
        }
        assert!(system_names.contains(&"Nervous System".to_string()));
        assert!(system_names.contains(&"Gastrointestinal System".to_string()));
    }

    #[tokio::test]
    async fn test_display() {
        let kg = build_magnesium_graph().await;
        let output = kg.dump().await;
        assert!(output.contains("Magnesium (Ingredient)"));
        assert!(output.contains("acts_on"));
        assert!(output.contains("Nervous System (System)"));
    }
}
