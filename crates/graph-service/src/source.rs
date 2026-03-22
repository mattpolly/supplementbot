use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use surrealdb::engine::local::Db;
use surrealdb::Surreal;
use surrealdb_types::SurrealValue;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Source tracking — who said what, when, and how many times.
//
// These are materialized projections of event log data, not primary storage.
// The JSONL event log is the portable source of truth. These tables exist
// for fast queries like "which providers have confirmed this edge?"
//
// Tables: node_source, edge_source (relational, not graph relations)
// ---------------------------------------------------------------------------

/// A single observation of a node by a provider.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct NodeObservation {
    /// The node name (matches graph node key)
    pub node_name: String,
    /// The node type as observed
    pub node_type: String,
    /// Which provider observed this node
    pub provider: String,
    /// Which model observed this node
    pub model: String,
    /// When the observation was recorded
    pub observed_at: String,
    /// Correlation ID linking back to the event log
    pub correlation_id: String,
}

/// A single observation of an edge by a provider — either creation or confirmation.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct EdgeObservation {
    /// Source node name
    pub source_node: String,
    /// Target node name
    pub target_node: String,
    /// Edge type
    pub edge_type: String,
    /// Confidence assigned by this observation
    pub confidence: f64,
    /// How this edge was produced (Extracted, StructurallyEmergent, Deduced)
    pub source_tag: String,
    /// Whether this was the initial creation or a confirmation of existing edge
    pub observation_type: String, // "created" or "confirmed"
    /// Which provider observed this edge
    pub provider: String,
    /// Which model observed this edge
    pub model: String,
    /// When the observation was recorded
    pub observed_at: String,
    /// Correlation ID linking back to the event log
    pub correlation_id: String,
}

/// Returned from the select (has SurrealDB's auto-generated id)
#[derive(Debug, Clone, SurrealValue)]
struct EdgeObservationWithId {
    #[allow(dead_code)]
    id: surrealdb_types::RecordId,
    pub source_node: String,
    pub target_node: String,
    pub edge_type: String,
    pub confidence: f64,
    pub source_tag: String,
    pub observation_type: String,
    pub provider: String,
    pub model: String,
    pub observed_at: String,
    pub correlation_id: String,
}

impl From<EdgeObservationWithId> for EdgeObservation {
    fn from(r: EdgeObservationWithId) -> Self {
        Self {
            source_node: r.source_node,
            target_node: r.target_node,
            edge_type: r.edge_type,
            confidence: r.confidence,
            source_tag: r.source_tag,
            observation_type: r.observation_type,
            provider: r.provider,
            model: r.model,
            observed_at: r.observed_at,
            correlation_id: r.correlation_id,
        }
    }
}

/// Summary of provider agreement for a specific edge.
#[derive(Debug, Clone)]
pub struct EdgeAgreement {
    /// How many distinct providers have observed this edge (created or confirmed)
    pub provider_count: usize,
    /// Which providers and their observation types
    pub providers: Vec<ProviderObservation>,
    /// How many total observations (including multiple from the same provider)
    pub total_observations: usize,
}

#[derive(Debug, Clone)]
pub struct ProviderObservation {
    pub provider: String,
    pub model: String,
    pub observation_type: String,
    pub confidence: f64,
}

// ---------------------------------------------------------------------------
// SourceStore — queryable projection of event log data
// ---------------------------------------------------------------------------

/// Tracks which providers observed which nodes and edges. Shares a SurrealDB
/// connection with the KnowledgeGraph — both live in the same database.
///
/// The source tables are materialized views of event log data.
/// The JSONL event log is the portable source of truth.
pub struct SourceStore {
    db: Surreal<Db>,
}

impl SourceStore {
    /// Create a source store using the same DB handle as the KnowledgeGraph.
    pub fn new(db: &Surreal<Db>) -> Self {
        Self { db: db.clone() }
    }

    // -- Write operations (projecting events into tables) ------------------

    /// Record that a node was observed by a provider.
    pub async fn record_node_observation(
        &self,
        node_name: &str,
        node_type: &str,
        provider: &str,
        model: &str,
        correlation_id: Uuid,
        observed_at: DateTime<Utc>,
    ) {
        let obs = NodeObservation {
            node_name: node_name.to_lowercase(),
            node_type: node_type.to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            observed_at: observed_at.to_rfc3339(),
            correlation_id: correlation_id.to_string(),
        };
        let _: Result<Option<NodeObservation>, _> = self.db.create("node_source").content(obs).await;
    }

    /// Record that an edge was created by a provider.
    pub async fn record_edge_created(
        &self,
        source_node: &str,
        target_node: &str,
        edge_type: &str,
        confidence: f64,
        source_tag: &str,
        provider: &str,
        model: &str,
        correlation_id: Uuid,
        observed_at: DateTime<Utc>,
    ) {
        let obs = EdgeObservation {
            source_node: source_node.to_lowercase(),
            target_node: target_node.to_lowercase(),
            edge_type: edge_type.to_string(),
            confidence,
            source_tag: source_tag.to_string(),
            observation_type: "created".to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            observed_at: observed_at.to_rfc3339(),
            correlation_id: correlation_id.to_string(),
        };
        let _: Result<Option<EdgeObservation>, _> = self.db.create("edge_source").content(obs).await;
    }

    /// Record that an existing edge was confirmed by a provider.
    pub async fn record_edge_confirmed(
        &self,
        source_node: &str,
        target_node: &str,
        edge_type: &str,
        provider: &str,
        model: &str,
        correlation_id: Uuid,
        observed_at: DateTime<Utc>,
    ) {
        let obs = EdgeObservation {
            source_node: source_node.to_lowercase(),
            target_node: target_node.to_lowercase(),
            edge_type: edge_type.to_string(),
            confidence: 0.0, // confirmation doesn't carry its own confidence
            source_tag: "Confirmed".to_string(),
            observation_type: "confirmed".to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            observed_at: observed_at.to_rfc3339(),
            correlation_id: correlation_id.to_string(),
        };
        let _: Result<Option<EdgeObservation>, _> = self.db.create("edge_source").content(obs).await;
    }

    // -- Query operations --------------------------------------------------

    /// Get all observations for a specific edge (created + confirmed).
    pub async fn observations_for_edge(
        &self,
        source_node: &str,
        target_node: &str,
        edge_type: &str,
    ) -> Vec<EdgeObservation> {
        let mut result = self
            .db
            .query(
                "SELECT * FROM edge_source WHERE source_node = $src AND target_node = $tgt AND edge_type = $et ORDER BY observed_at ASC",
            )
            .bind(("src", source_node.to_lowercase()))
            .bind(("tgt", target_node.to_lowercase()))
            .bind(("et", edge_type.to_string()))
            .await
            .unwrap();
        let records: Vec<EdgeObservationWithId> = result.take(0).unwrap_or_default();
        records.into_iter().map(EdgeObservation::from).collect()
    }

    /// Get provider agreement summary for a specific edge.
    pub async fn provider_agreement(
        &self,
        source_node: &str,
        target_node: &str,
        edge_type: &str,
    ) -> EdgeAgreement {
        let observations = self
            .observations_for_edge(source_node, target_node, edge_type)
            .await;

        let providers: Vec<ProviderObservation> = observations
            .iter()
            .map(|o| ProviderObservation {
                provider: o.provider.clone(),
                model: o.model.clone(),
                observation_type: o.observation_type.clone(),
                confidence: o.confidence,
            })
            .collect();

        let unique_providers: std::collections::HashSet<&str> =
            providers.iter().map(|p| p.provider.as_str()).collect();

        EdgeAgreement {
            provider_count: unique_providers.len(),
            total_observations: observations.len(),
            providers,
        }
    }

    /// Count total edge observations across all edges.
    pub async fn total_edge_observations(&self) -> usize {
        let mut result = self
            .db
            .query("SELECT count() FROM edge_source GROUP ALL")
            .await
            .unwrap();
        let counts: Vec<CountResult> = result.take(0).unwrap_or_default();
        counts.into_iter().next().map(|c| c.count).unwrap_or(0)
    }

    /// Count total node observations across all nodes.
    pub async fn total_node_observations(&self) -> usize {
        let mut result = self
            .db
            .query("SELECT count() FROM node_source GROUP ALL")
            .await
            .unwrap();
        let counts: Vec<CountResult> = result.take(0).unwrap_or_default();
        counts.into_iter().next().map(|c| c.count).unwrap_or(0)
    }

    /// Get all edges that have been observed by multiple providers.
    pub async fn multi_provider_edges(&self) -> Vec<MultiProviderEdge> {
        // Get distinct (source, target, edge_type, provider) combinations
        let mut result = self
            .db
            .query(
                "SELECT source_node, target_node, edge_type, array::distinct(provider) AS providers \
                 FROM edge_source \
                 GROUP BY source_node, target_node, edge_type",
            )
            .await
            .unwrap();
        let records: Vec<GroupedEdge> = result.take(0).unwrap_or_default();

        records
            .into_iter()
            .filter(|r| r.providers.len() > 1)
            .map(|r| MultiProviderEdge {
                source_node: r.source_node,
                target_node: r.target_node,
                edge_type: r.edge_type,
                providers: r.providers,
            })
            .collect()
    }
}

#[derive(Debug, SurrealValue)]
struct CountResult {
    count: usize,
}

#[derive(Debug, SurrealValue)]
struct GroupedEdge {
    source_node: String,
    target_node: String,
    edge_type: String,
    providers: Vec<String>,
}

/// An edge that has been independently observed by multiple providers.
#[derive(Debug, Clone)]
pub struct MultiProviderEdge {
    pub source_node: String,
    pub target_node: String,
    pub edge_type: String,
    pub providers: Vec<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::KnowledgeGraph;

    async fn make_store() -> SourceStore {
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        SourceStore::new(kg.db())
    }

    #[tokio::test]
    async fn test_record_and_query_edge_observation() {
        let store = make_store().await;
        let corr = Uuid::new_v4();
        let now = Utc::now();

        store
            .record_edge_created(
                "magnesium",
                "muscular system",
                "acts_on",
                0.7,
                "Extracted",
                "anthropic",
                "claude-sonnet",
                corr,
                now,
            )
            .await;

        let obs = store
            .observations_for_edge("magnesium", "muscular system", "acts_on")
            .await;

        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].provider, "anthropic");
        assert_eq!(obs[0].observation_type, "created");
        assert_eq!(obs[0].confidence, 0.7);
    }

    #[tokio::test]
    async fn test_edge_confirmed_by_second_provider() {
        let store = make_store().await;
        let corr1 = Uuid::new_v4();
        let corr2 = Uuid::new_v4();
        let now = Utc::now();

        // Anthropic creates the edge
        store
            .record_edge_created(
                "magnesium",
                "nervous system",
                "acts_on",
                0.7,
                "Extracted",
                "anthropic",
                "claude-sonnet",
                corr1,
                now,
            )
            .await;

        // Gemini confirms it
        store
            .record_edge_confirmed(
                "magnesium",
                "nervous system",
                "acts_on",
                "gemini",
                "gemini-flash",
                corr2,
                now,
            )
            .await;

        let agreement = store
            .provider_agreement("magnesium", "nervous system", "acts_on")
            .await;

        assert_eq!(agreement.provider_count, 2);
        assert_eq!(agreement.total_observations, 2);
    }

    #[tokio::test]
    async fn test_same_provider_multiple_observations() {
        let store = make_store().await;
        let corr1 = Uuid::new_v4();
        let corr2 = Uuid::new_v4();
        let now = Utc::now();

        // Anthropic creates the edge
        store
            .record_edge_created(
                "zinc",
                "immune system",
                "acts_on",
                0.7,
                "Extracted",
                "anthropic",
                "claude-sonnet",
                corr1,
                now,
            )
            .await;

        // Anthropic confirms it again in comprehension check
        store
            .record_edge_confirmed(
                "zinc",
                "immune system",
                "acts_on",
                "anthropic",
                "claude-sonnet",
                corr2,
                now,
            )
            .await;

        let agreement = store
            .provider_agreement("zinc", "immune system", "acts_on")
            .await;

        // Same provider, so only 1 unique provider
        assert_eq!(agreement.provider_count, 1);
        // But 2 total observations
        assert_eq!(agreement.total_observations, 2);
    }

    #[tokio::test]
    async fn test_multi_provider_edges() {
        let store = make_store().await;
        let now = Utc::now();

        // Edge observed by both providers
        store
            .record_edge_created(
                "magnesium", "muscular system", "acts_on", 0.7, "Extracted",
                "anthropic", "claude-sonnet", Uuid::new_v4(), now,
            )
            .await;
        store
            .record_edge_confirmed(
                "magnesium", "muscular system", "acts_on",
                "gemini", "gemini-flash", Uuid::new_v4(), now,
            )
            .await;

        // Edge observed by only one provider
        store
            .record_edge_created(
                "zinc", "skin repair", "affords", 0.5, "StructurallyEmergent",
                "anthropic", "claude-sonnet", Uuid::new_v4(), now,
            )
            .await;

        let multi = store.multi_provider_edges().await;

        assert_eq!(multi.len(), 1);
        assert_eq!(multi[0].source_node, "magnesium");
        assert_eq!(multi[0].providers.len(), 2);
    }

    #[tokio::test]
    async fn test_observation_counts() {
        let store = make_store().await;
        let now = Utc::now();

        store
            .record_node_observation(
                "magnesium", "Ingredient", "anthropic", "claude-sonnet",
                Uuid::new_v4(), now,
            )
            .await;
        store
            .record_node_observation(
                "muscular system", "System", "anthropic", "claude-sonnet",
                Uuid::new_v4(), now,
            )
            .await;
        store
            .record_edge_created(
                "magnesium", "muscular system", "acts_on", 0.7, "Extracted",
                "anthropic", "claude-sonnet", Uuid::new_v4(), now,
            )
            .await;

        assert_eq!(store.total_node_observations().await, 2);
        assert_eq!(store.total_edge_observations().await, 1);
    }
}
