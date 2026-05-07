use sqlx::PgPool;
use surrealdb::engine::any::Any;
use surrealdb::opt::auth::Root;
use surrealdb::Surreal;
use uuid::Uuid;

use crate::export::{ExportEdge, ExportGraph, ExportNode};
use crate::types::*;

// ---------------------------------------------------------------------------
// NodeIndex — opaque handle to an entity in supplementology Postgres.
// Wraps a UUID. The old SurrealDB RecordId is gone.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeIndex(pub Uuid);

impl NodeIndex {
    pub fn id(&self) -> Uuid {
        self.0
    }

    pub fn default_for_test() -> Self {
        Self(Uuid::nil())
    }
}

// ---------------------------------------------------------------------------
// Internal row types for sqlx queries
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct EntityRow {
    #[allow(dead_code)]
    id: Uuid,
    name: String,
    type_name: String,
}

#[allow(dead_code)]
#[derive(sqlx::FromRow)]
struct RelationshipRow {
    id: Uuid,
    from_entity: Uuid,
    to_entity: Uuid,
    rel_type_name: String,
    confidence: f32,
    complexity: f32,
    source: String,
    attrs: serde_json::Value,
}

// ---------------------------------------------------------------------------
// KnowledgeGraph — Postgres-backed supplement KB.
// Holds the PgPool for supplement data + a SurrealDB handle for the intake
// graph and explore endpoints (which remain on SurrealDB).
// ---------------------------------------------------------------------------

pub struct KnowledgeGraph {
    pub(crate) pool: PgPool,
    /// Retained for intake graph store, iDISK, and explore endpoints.
    db: Surreal<Any>,
}

impl KnowledgeGraph {
    /// Connect to supplementology Postgres (supplement KB) and SurrealDB (intake graph).
    /// `pg_url` is the Postgres connection string.
    /// `surreal_url/user/pass` are the SurrealDB credentials (intake graph only).
    pub async fn open(
        pg_url: &str,
        surreal_url: &str,
        surreal_user: &str,
        surreal_pass: &str,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let pool = PgPool::connect(pg_url).await?;
        let db = surrealdb::engine::any::connect(surreal_url).await?;
        db.signin(Root {
            username: surreal_user.to_string(),
            password: surreal_pass.to_string(),
        })
        .await?;
        db.use_ns("supplementbot").use_db("supplementbot").await?;
        // Ensure intake graph relation table exists
        let _: surrealdb::Result<Vec<serde_json::Value>> = db
            .query("DEFINE TABLE IF NOT EXISTS edge TYPE RELATION IN node OUT node")
            .await
            .and_then(|mut r| r.take(0));
        Ok(Self { pool, db })
    }

    /// Create an in-memory graph for tests (Postgres only — no intake graph).
    pub async fn in_memory() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let pool = PgPool::connect(
            &std::env::var("SUPPLEMENTOLOGY_DATABASE_URL")
                .unwrap_or_else(|_| "postgresql://supplementology:supplementology@localhost:5433/supplementology".to_string()),
        )
        .await?;
        let db = surrealdb::engine::any::connect("memory").await?;
        db.use_ns("supplementbot").use_db("supplementbot").await?;
        Ok(Self { pool, db })
    }

    /// Get the SurrealDB handle (for intake graph store, iDISK, explore endpoints).
    pub fn db(&self) -> &Surreal<Any> {
        &self.db
    }

    /// Get the Postgres pool (for SourceStore, MergeStore, IngredientRegistry).
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    // -- Node operations --------------------------------------------------

    /// Add a node. If a node with this name already exists, returns the existing index.
    pub async fn add_node(&self, data: NodeData) -> NodeIndex {
        if let Some(idx) = self.find_node(&data.name).await {
            return idx;
        }

        let type_name = node_type_to_pg(&data.node_type);
        let slug = slugify(&data.name);

        // Try insert; if slug collision just look up
        let result = sqlx::query_scalar!(
            r#"
            WITH et AS (SELECT id FROM entity_type WHERE name = $1)
            INSERT INTO entity (type_id, name, slug, source, attrs)
            SELECT et.id, $2, $3, 'nsai_graph', '{}'
            FROM et
            ON CONFLICT (type_id, slug) DO NOTHING
            RETURNING id
            "#,
            type_name, data.name, slug,
        )
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();

        if let Some(id) = result {
            return NodeIndex(id);
        }

        // Already exists — fetch it
        self.find_node(&data.name).await.unwrap_or(NodeIndex(Uuid::new_v4()))
    }

    /// Look up a node by name (case-insensitive, checks entity name then synonyms).
    pub async fn find_node(&self, name: &str) -> Option<NodeIndex> {
        // Check entity name first
        let row = sqlx::query_scalar!(
            "SELECT e.id FROM entity e WHERE LOWER(e.name) = LOWER($1) LIMIT 1",
            name,
        )
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();

        if let Some(id) = row {
            return Some(NodeIndex(id));
        }

        // Check synonyms
        sqlx::query_scalar!(
            r#"
            SELECT e.id FROM synonym s
            JOIN entity e ON e.id = s.entity_id
            WHERE LOWER(s.name) = LOWER($1)
            AND e.source = 'seed'
            LIMIT 1
            "#,
            name,
        )
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .map(NodeIndex)
    }

    /// Get node data by index.
    pub async fn node_data(&self, idx: &NodeIndex) -> Option<NodeData> {
        let row = sqlx::query_as!(
            EntityRow,
            r#"
            SELECT e.id, e.name, et.name AS type_name
            FROM entity e
            JOIN entity_type et ON et.id = e.type_id
            WHERE e.id = $1
            "#,
            idx.0,
        )
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()?;

        let node_type = pg_to_node_type(&row.type_name)?;
        Some(NodeData::new(row.name, node_type))
    }

    /// Get all nodes of a given type.
    pub async fn nodes_by_type(&self, node_type: &NodeType) -> Vec<NodeIndex> {
        let type_name = node_type_to_pg(node_type);
        sqlx::query_scalar!(
            r#"
            SELECT e.id FROM entity e
            JOIN entity_type et ON et.id = e.type_id
            WHERE et.name = $1
            "#,
            type_name,
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(NodeIndex)
        .collect()
    }

    /// Return all seeded ingredient names, sorted alphabetically.
    pub async fn known_ingredients(&self) -> Vec<String> {
        sqlx::query_scalar!(
            r#"
            SELECT e.name FROM entity e
            JOIN entity_type et ON et.id = e.type_id
            WHERE et.name IN ('organism', 'compound')
              AND e.source = 'seed'
              AND e.slug != 'dsld_ingredient_registry'
            ORDER BY e.name ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }

    /// Find a node by name, with alias fallback via MergeStore.
    pub async fn find_node_or_alias(
        &self,
        name: &str,
        merge: &crate::merge::MergeStore,
    ) -> Option<NodeIndex> {
        if let Some(idx) = self.find_node(name).await {
            return Some(idx);
        }
        let canonical = merge.resolve(name).await;
        if canonical != name.to_lowercase() {
            self.find_node(&canonical).await
        } else {
            None
        }
    }

    // -- Edge operations --------------------------------------------------

    /// Add an edge between two nodes. Idempotent on (from, to, rel_type).
    pub async fn add_edge(&self, source: &NodeIndex, target: &NodeIndex, data: EdgeData) {
        let rel_type_name = edge_type_to_pg(&data.edge_type);
        let complexity = data.edge_type.min_complexity() as f32;
        let source_str = format!("{:?}", data.metadata.source).to_lowercase();
        let pg_source = match data.metadata.source {
            Source::Extracted => "nsai_extracted",
            Source::StructurallyEmergent => "nsai_emergent",
            Source::Deduced => "nsai_deduced",
        };
        let attrs = serde_json::json!({
            "epoch": data.metadata.epoch,
            "iteration": data.metadata.iteration,
            "reasoning_depth": data.metadata.reasoning_depth,
        });
        let _ = sqlx::query!(
            r#"
            INSERT INTO relationship (rel_type_id, from_entity, to_entity, confidence, complexity, source, attrs)
            SELECT rt.id, $2, $3, $4, $5, $6, $7
            FROM rel_type rt WHERE rt.name = $1
            ON CONFLICT DO NOTHING
            "#,
            rel_type_name,
            source.0,
            target.0,
            data.metadata.confidence as f32,
            complexity,
            pg_source,
            attrs,
        )
        .execute(&self.pool)
        .await;
        let _ = source_str; // suppress unused warning
    }

    /// Get all outgoing edges from a node.
    pub async fn outgoing_edges(&self, idx: &NodeIndex) -> Vec<(NodeIndex, EdgeData)> {
        self.fetch_edges_where("r.from_entity = $1", idx.0).await
    }

    /// Get all incoming edges to a node.
    pub async fn incoming_edges(&self, idx: &NodeIndex) -> Vec<(NodeIndex, EdgeData)> {
        self.fetch_edges_where("r.to_entity = $1", idx.0).await
    }

    async fn fetch_edges_where(&self, condition: &str, entity_id: Uuid) -> Vec<(NodeIndex, EdgeData)> {
        let rows = sqlx::query!(
            r#"
            SELECT r.id, r.from_entity, r.to_entity,
                   rt.name AS rel_type_name,
                   r.confidence, r.complexity, r.source, r.attrs
            FROM relationship r
            JOIN rel_type rt ON rt.id = r.rel_type_id
            WHERE r.from_entity = $1 OR r.to_entity = $1
            ORDER BY r.confidence DESC
            "#,
            entity_id,
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        let is_outgoing = condition.contains("from_entity");

        rows.into_iter()
            .filter(|r| {
                if is_outgoing { r.from_entity == entity_id }
                else { r.to_entity == entity_id }
            })
            .filter_map(|r| {
                let edge_type = pg_to_edge_type(&r.rel_type_name)?;
                let other = if is_outgoing { r.to_entity } else { r.from_entity };
                let metadata = EdgeMetadata {
                    confidence: r.confidence as f64,
                    source: pg_source_to_source(&r.source),
                    iteration: r.attrs.get("iteration").and_then(|v| v.as_u64()).unwrap_or(1) as u32,
                    epoch: r.attrs.get("epoch").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                    llm_agreement: None,
                    reasoning_depth: r.attrs.get("reasoning_depth").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                    extra: std::collections::HashMap::new(),
                };
                Some((NodeIndex(other), EdgeData::new(edge_type, metadata)))
            })
            .collect()
    }

    /// Total degree (incoming + outgoing) for a node.
    pub async fn node_degree(&self, idx: &NodeIndex) -> usize {
        sqlx::query_scalar!(
            "SELECT COUNT(*) FROM relationship WHERE from_entity = $1 OR to_entity = $1",
            idx.0,
        )
        .fetch_one(&self.pool)
        .await
        .ok()
        .flatten()
        .unwrap_or(0) as usize
    }

    /// Boost edge confidence by `boost`, capped at 1.0. Returns updated count.
    pub async fn boost_edge_confidence(
        &self,
        source: &NodeIndex,
        target: &NodeIndex,
        edge_type: &EdgeType,
        boost: f64,
    ) -> usize {
        let rel_type_name = edge_type_to_pg(edge_type);
        let result = sqlx::query!(
            r#"
            UPDATE relationship SET
                confidence = LEAST(1.0, confidence + $4)
            WHERE from_entity = $1 AND to_entity = $2
              AND rel_type_id = (SELECT id FROM rel_type WHERE name = $3)
            "#,
            source.0, target.0, rel_type_name, boost as f32,
        )
        .execute(&self.pool)
        .await;
        result.map(|r| r.rows_affected() as usize).unwrap_or(0)
    }

    /// Set edge confidence to exact value. Returns updated count.
    pub async fn set_edge_confidence(
        &self,
        source: &NodeIndex,
        target: &NodeIndex,
        edge_type: &EdgeType,
        confidence: f64,
    ) -> usize {
        let rel_type_name = edge_type_to_pg(edge_type);
        let result = sqlx::query!(
            r#"
            UPDATE relationship SET confidence = $4
            WHERE from_entity = $1 AND to_entity = $2
              AND rel_type_id = (SELECT id FROM rel_type WHERE name = $3)
            "#,
            source.0, target.0, rel_type_name, confidence.clamp(0.0, 1.0) as f32,
        )
        .execute(&self.pool)
        .await;
        result.map(|r| r.rows_affected() as usize).unwrap_or(0)
    }

    pub async fn all_nodes(&self) -> Vec<NodeIndex> {
        sqlx::query_scalar!("SELECT id FROM entity")
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(NodeIndex)
            .collect()
    }

    pub async fn all_edges(&self) -> Vec<(NodeIndex, NodeIndex, EdgeData)> {
        let rows = sqlx::query!(
            r#"
            SELECT r.from_entity, r.to_entity, rt.name AS rel_type_name,
                   r.confidence, r.source, r.attrs
            FROM relationship r JOIN rel_type rt ON rt.id = r.rel_type_id
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        rows.into_iter()
            .filter_map(|r| {
                let edge_type = pg_to_edge_type(&r.rel_type_name)?;
                let metadata = EdgeMetadata {
                    confidence: r.confidence as f64,
                    source: pg_source_to_source(&r.source),
                    iteration: 1,
                    epoch: 0,
                    llm_agreement: None,
                    reasoning_depth: 0,
                    extra: std::collections::HashMap::new(),
                };
                Some((
                    NodeIndex(r.from_entity),
                    NodeIndex(r.to_entity),
                    EdgeData::new(edge_type, metadata),
                ))
            })
            .collect()
    }

    pub async fn node_count(&self) -> usize {
        sqlx::query_scalar!("SELECT COUNT(*) FROM entity")
            .fetch_one(&self.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(0) as usize
    }

    pub async fn edge_count(&self) -> usize {
        sqlx::query_scalar!("SELECT COUNT(*) FROM relationship")
            .fetch_one(&self.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(0) as usize
    }

    pub async fn export_json(&self) -> ExportGraph {
        let node_rows = sqlx::query!(
            "SELECT e.id, e.name, et.name AS type_name FROM entity e JOIN entity_type et ON et.id = e.type_id"
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        let nodes: Vec<ExportNode> = node_rows
            .iter()
            .map(|n| ExportNode {
                id: n.id.to_string(),
                name: n.name.clone(),
                node_type: n.type_name.clone(),
            })
            .collect();

        let edge_rows = sqlx::query!(
            r#"
            SELECT r.from_entity, r.to_entity, rt.name AS rel_type_name,
                   r.confidence, r.source
            FROM relationship r JOIN rel_type rt ON rt.id = r.rel_type_id
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        let edges: Vec<ExportEdge> = edge_rows
            .iter()
            .map(|e| ExportEdge {
                source: e.from_entity.to_string(),
                target: e.to_entity.to_string(),
                edge_type: e.rel_type_name.clone(),
                confidence: e.confidence as f64,
                source_tag: e.source.clone(),
            })
            .collect();

        ExportGraph { nodes, edges }
    }

    pub async fn dump(&self) -> String {
        let node_count = self.node_count().await;
        let edge_count = self.edge_count().await;
        format!(
            "KnowledgeGraph ({} entities, {} relationships) — backed by supplementology Postgres\n",
            node_count, edge_count
        )
    }
}

// ---------------------------------------------------------------------------
// Type conversion helpers
// ---------------------------------------------------------------------------

fn node_type_to_pg(nt: &NodeType) -> &'static str {
    match nt {
        NodeType::Ingredient => "organism",
        NodeType::System => "system",
        NodeType::Mechanism => "mechanism",
        NodeType::Symptom => "symptom",
        NodeType::Property => "property",
        NodeType::Condition => "condition",
        NodeType::Substrate => "substrate",
        NodeType::Pathway => "pathway",
        NodeType::BiologicalProcess => "biological_process",
        NodeType::Metabolite => "metabolite",
        NodeType::GeneProtein => "gene_protein",
        NodeType::CellType => "cell_type",
        NodeType::Microbiota => "microbiota",
        NodeType::Receptor => "receptor",
    }
}

fn pg_to_node_type(s: &str) -> Option<NodeType> {
    match s {
        "organism" | "compound" => Some(NodeType::Ingredient),
        "system" => Some(NodeType::System),
        "mechanism" => Some(NodeType::Mechanism),
        "symptom" => Some(NodeType::Symptom),
        "property" => Some(NodeType::Property),
        "condition" => Some(NodeType::Condition),
        "substrate" => Some(NodeType::Substrate),
        "pathway" => Some(NodeType::Pathway),
        "biological_process" => Some(NodeType::BiologicalProcess),
        "metabolite" => Some(NodeType::Metabolite),
        "gene_protein" => Some(NodeType::GeneProtein),
        "cell_type" => Some(NodeType::CellType),
        "microbiota" => Some(NodeType::Microbiota),
        "receptor" => Some(NodeType::Receptor),
        _ => None,
    }
}

fn edge_type_to_pg(et: &EdgeType) -> &'static str {
    match et {
        EdgeType::ActsOn => "acts_on",
        EdgeType::ViaMechanism => "via_mechanism",
        EdgeType::Affords => "affords",
        EdgeType::PresentsIn => "presents_in",
        EdgeType::Modulates => "modulates",
        EdgeType::ContraindicatedWith => "contraindicated_with",
        EdgeType::CompetesWith => "competes_with",
        EdgeType::Disinhibits => "disinhibits",
        EdgeType::Sequesters => "sequesters",
        EdgeType::Releases => "releases",
        EdgeType::Amplifies => "amplifies",
        EdgeType::Desensitizes => "desensitizes",
        EdgeType::PositivelyReinforces => "positively_reinforces",
        EdgeType::Gates => "gates",
    }
}

fn pg_to_edge_type(s: &str) -> Option<EdgeType> {
    match s {
        "acts_on" => Some(EdgeType::ActsOn),
        "via_mechanism" => Some(EdgeType::ViaMechanism),
        "affords" => Some(EdgeType::Affords),
        "presents_in" => Some(EdgeType::PresentsIn),
        "modulates" => Some(EdgeType::Modulates),
        "contraindicated_with" => Some(EdgeType::ContraindicatedWith),
        "competes_with" => Some(EdgeType::CompetesWith),
        "disinhibits" => Some(EdgeType::Disinhibits),
        "sequesters" => Some(EdgeType::Sequesters),
        "releases" => Some(EdgeType::Releases),
        "amplifies" => Some(EdgeType::Amplifies),
        "desensitizes" => Some(EdgeType::Desensitizes),
        "positively_reinforces" => Some(EdgeType::PositivelyReinforces),
        "gates" => Some(EdgeType::Gates),
        _ => None,
    }
}

fn pg_source_to_source(s: &str) -> Source {
    match s {
        "nsai_emergent" => Source::StructurallyEmergent,
        "nsai_deduced" => Source::Deduced,
        _ => Source::Extracted,
    }
}

pub(crate) fn slugify(name: &str) -> String {
    name.trim()
        .to_lowercase()
        .replace(' ', "_")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect()
}

// ---------------------------------------------------------------------------
// Tests (integration — require SUPPLEMENTOLOGY_DATABASE_URL env var)
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_known_ingredients_returns_seeds() {
        if std::env::var("SUPPLEMENTOLOGY_DATABASE_URL").is_err() {
            return;
        }
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        let ingredients = kg.known_ingredients().await;
        assert!(!ingredients.is_empty(), "expected seeded ingredients");
        assert!(ingredients.contains(&"Magnesium".to_string()) ||
                ingredients.iter().any(|n| n.to_lowercase().contains("magnesium")));
    }

    #[tokio::test]
    async fn test_find_node_by_name() {
        if std::env::var("SUPPLEMENTOLOGY_DATABASE_URL").is_err() {
            return;
        }
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        // Magnesium should exist as a seeded ingredient
        let idx = kg.find_node("Magnesium").await;
        assert!(idx.is_some(), "Magnesium should be findable");
    }

    #[tokio::test]
    async fn test_outgoing_edges_for_ingredient() {
        if std::env::var("SUPPLEMENTOLOGY_DATABASE_URL").is_err() {
            return;
        }
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        if let Some(idx) = kg.find_node("magnesium").await {
            let edges = kg.outgoing_edges(&idx).await;
            // Magnesium has NSAI edges — we just check it doesn't panic
            let _ = edges;
        }
    }
}
