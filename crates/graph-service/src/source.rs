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

/// A PubMed citation backing a specific edge, sourced from SuppKG.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct CitationRecord {
    /// Our graph's source node name
    pub source_node: String,
    /// Our graph's target node name
    pub target_node: String,
    /// Our graph's edge type
    pub edge_type: String,
    /// PubMed ID
    pub pmid: String,
    /// Supporting sentence from the abstract
    pub sentence: String,
    /// SuppKG's confidence for this citation
    pub confidence: f64,
    /// The SuppKG predicate that matched (e.g. AFFECTS, STIMULATES)
    pub suppkg_predicate: String,
    /// CUI that matched our source node
    pub source_cui: String,
    /// CUI that matched our target node
    pub target_cui: String,
}

#[derive(Debug, Clone, SurrealValue)]
struct CitationRecordWithId {
    #[allow(dead_code)]
    id: surrealdb_types::RecordId,
    pub source_node: String,
    pub target_node: String,
    pub edge_type: String,
    pub pmid: String,
    pub sentence: String,
    pub confidence: f64,
    pub suppkg_predicate: String,
    pub source_cui: String,
    pub target_cui: String,
}

impl From<CitationRecordWithId> for CitationRecord {
    fn from(r: CitationRecordWithId) -> Self {
        Self {
            source_node: r.source_node,
            target_node: r.target_node,
            edge_type: r.edge_type,
            pmid: r.pmid,
            sentence: r.sentence,
            confidence: r.confidence,
            suppkg_predicate: r.suppkg_predicate,
            source_cui: r.source_cui,
            target_cui: r.target_cui,
        }
    }
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

    /// Classify all observed edges by quality tier.
    ///
    /// Quality is derived from source observations:
    /// - Deduced: only `system:forward_chain` observations
    /// - Speculative: only `StructurallyEmergent` source tags
    /// - SingleProvider: extracted by exactly one LLM provider
    /// - MultiProvider: extracted by 2+ independent LLM providers
    pub async fn edges_by_quality(&self) -> Vec<EdgeWithQuality> {
        let mut result = self
            .db
            .query(
                "SELECT source_node, target_node, edge_type, \
                 array::distinct(provider) AS providers, \
                 array::distinct(source_tag) AS source_tags, \
                 count() AS total_obs \
                 FROM edge_source \
                 GROUP BY source_node, target_node, edge_type",
            )
            .await
            .unwrap();
        let records: Vec<QualityGroupedEdge> = result.take(0).unwrap_or_default();

        // Build set of edges that have citations
        let cited: std::collections::HashSet<(String, String, String)> = self
            .all_citations()
            .await
            .into_iter()
            .map(|c| (c.source_node, c.target_node, c.edge_type))
            .collect();

        records
            .into_iter()
            .map(|r| {
                // Filter out system providers to count real LLM providers
                let llm_providers: Vec<&String> = r
                    .providers
                    .iter()
                    .filter(|p| !p.starts_with("system:"))
                    .collect();

                let has_citation = cited.contains(&(
                    r.source_node.clone(),
                    r.target_node.clone(),
                    r.edge_type.clone(),
                ));

                let quality = if has_citation {
                    EdgeQuality::CitationBacked
                } else if llm_providers.len() >= 2 {
                    EdgeQuality::MultiProvider
                } else if llm_providers.len() == 1 {
                    // Check if this is purely speculative
                    let all_speculative = r.source_tags.iter().all(|t| {
                        t == "StructurallyEmergent" || t == "Confirmed"
                    });
                    if all_speculative {
                        EdgeQuality::Speculative
                    } else {
                        EdgeQuality::SingleProvider
                    }
                } else {
                    // Only system providers (forward chain)
                    EdgeQuality::Deduced
                };

                EdgeWithQuality {
                    source_node: r.source_node,
                    target_node: r.target_node,
                    edge_type: r.edge_type,
                    quality,
                    provider_count: llm_providers.len(),
                    total_observations: r.total_obs,
                }
            })
            .collect()
    }

    /// Get all edges at or above a given quality tier.
    pub async fn edges_at_quality(&self, min_quality: EdgeQuality) -> Vec<EdgeWithQuality> {
        self.edges_by_quality()
            .await
            .into_iter()
            .filter(|e| e.quality >= min_quality)
            .collect()
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

    // -- Citation methods -----------------------------------------------------

    /// Record a PubMed citation backing a specific edge.
    pub async fn record_citation(&self, citation: &CitationRecord) {
        let _: Result<Option<CitationRecord>, _> = self
            .db
            .create("edge_citation")
            .content(citation.clone())
            .await;
    }

    /// Get all citations for a specific edge.
    pub async fn citations_for_edge(
        &self,
        source_node: &str,
        target_node: &str,
        edge_type: &str,
    ) -> Vec<CitationRecord> {
        let results: Vec<CitationRecordWithId> = self
            .db
            .query(
                "SELECT * FROM edge_citation WHERE source_node = $src AND target_node = $tgt AND edge_type = $et",
            )
            .bind(("src", source_node.to_lowercase()))
            .bind(("tgt", target_node.to_lowercase()))
            .bind(("et", edge_type.to_string()))
            .await
            .unwrap()
            .take(0)
            .unwrap_or_default();

        results.into_iter().map(CitationRecord::from).collect()
    }

    /// Get all citations in the store.
    pub async fn all_citations(&self) -> Vec<CitationRecord> {
        let results: Vec<CitationRecordWithId> = self
            .db
            .query("SELECT * FROM edge_citation")
            .await
            .unwrap()
            .take(0)
            .unwrap_or_default();

        results.into_iter().map(CitationRecord::from).collect()
    }

    /// Count total citations.
    pub async fn citation_count(&self) -> usize {
        let results: Vec<CountResult> = self
            .db
            .query("SELECT count() AS count FROM edge_citation GROUP ALL")
            .await
            .unwrap()
            .take(0)
            .unwrap_or_default();

        results.first().map(|r| r.count).unwrap_or(0)
    }

    /// Count unique edges that have at least one citation.
    pub async fn cited_edge_count(&self) -> usize {
        let results: Vec<CountResult> = self
            .db
            .query("SELECT count() AS count FROM (SELECT source_node, target_node, edge_type FROM edge_citation GROUP BY source_node, target_node, edge_type)")
            .await
            .unwrap()
            .take(0)
            .unwrap_or_default();

        results.first().map(|r| r.count).unwrap_or(0)
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

#[derive(Debug, SurrealValue)]
struct QualityGroupedEdge {
    source_node: String,
    target_node: String,
    edge_type: String,
    providers: Vec<String>,
    source_tags: Vec<String>,
    total_obs: usize,
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
// Edge quality tiers
//
// Quality is derived from source observations, not stored on the edge itself.
// The graph topology stays complete; consumers query the source layer to
// filter by quality when they need to.
// ---------------------------------------------------------------------------

/// Quality tier for an edge, derived from its source observations.
/// Ordered from weakest to strongest evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EdgeQuality {
    /// Deduced by forward chaining from other edges (no direct observation)
    Deduced,
    /// Inferred from graph topology, validated by LLM (speculative)
    Speculative,
    /// Extracted by a single LLM provider
    SingleProvider,
    /// Extracted by multiple independent LLM providers
    MultiProvider,
    /// Backed by at least one PubMed citation via SuppKG
    CitationBacked,
}

impl EdgeQuality {
    pub fn label(&self) -> &'static str {
        match self {
            EdgeQuality::Deduced => "deduced",
            EdgeQuality::Speculative => "speculative",
            EdgeQuality::SingleProvider => "single_provider",
            EdgeQuality::MultiProvider => "multi_provider",
            EdgeQuality::CitationBacked => "citation_backed",
        }
    }
}

/// An edge with its computed quality tier
#[derive(Debug, Clone)]
pub struct EdgeWithQuality {
    pub source_node: String,
    pub target_node: String,
    pub edge_type: String,
    pub quality: EdgeQuality,
    pub provider_count: usize,
    pub total_observations: usize,
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

    #[tokio::test]
    async fn test_edges_by_quality_single_provider() {
        let store = make_store().await;
        let now = Utc::now();

        store
            .record_edge_created(
                "magnesium", "muscular system", "acts_on", 0.7, "Extracted",
                "anthropic", "claude-sonnet", Uuid::new_v4(), now,
            )
            .await;

        let edges = store.edges_by_quality().await;
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].quality, EdgeQuality::SingleProvider);
        assert_eq!(edges[0].provider_count, 1);
    }

    #[tokio::test]
    async fn test_edges_by_quality_multi_provider() {
        let store = make_store().await;
        let now = Utc::now();

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

        let edges = store.edges_by_quality().await;
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].quality, EdgeQuality::MultiProvider);
        assert_eq!(edges[0].provider_count, 2);
    }

    #[tokio::test]
    async fn test_edges_by_quality_speculative() {
        let store = make_store().await;
        let now = Utc::now();

        store
            .record_edge_created(
                "magnesium", "immune support", "affords", 0.5, "StructurallyEmergent",
                "anthropic", "claude-sonnet", Uuid::new_v4(), now,
            )
            .await;

        let edges = store.edges_by_quality().await;
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].quality, EdgeQuality::Speculative);
    }

    #[tokio::test]
    async fn test_edges_by_quality_deduced() {
        let store = make_store().await;
        let now = Utc::now();

        store
            .record_edge_created(
                "magnesium", "muscle relaxation", "affords", 0.7, "Deduced",
                "system:forward_chain", "forward_chain", Uuid::new_v4(), now,
            )
            .await;

        let edges = store.edges_by_quality().await;
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].quality, EdgeQuality::Deduced);
    }

    #[tokio::test]
    async fn test_edges_at_quality_filters() {
        let store = make_store().await;
        let now = Utc::now();

        // Deduced edge
        store
            .record_edge_created(
                "magnesium", "muscle relaxation", "affords", 0.7, "Deduced",
                "system:forward_chain", "forward_chain", Uuid::new_v4(), now,
            )
            .await;

        // Single-provider extracted edge
        store
            .record_edge_created(
                "magnesium", "muscular system", "acts_on", 0.7, "Extracted",
                "anthropic", "claude-sonnet", Uuid::new_v4(), now,
            )
            .await;

        // Multi-provider edge
        store
            .record_edge_created(
                "zinc", "immune system", "acts_on", 0.7, "Extracted",
                "anthropic", "claude-sonnet", Uuid::new_v4(), now,
            )
            .await;
        store
            .record_edge_confirmed(
                "zinc", "immune system", "acts_on",
                "gemini", "gemini-flash", Uuid::new_v4(), now,
            )
            .await;

        // Filter: at least SingleProvider
        let filtered = store.edges_at_quality(EdgeQuality::SingleProvider).await;
        assert_eq!(filtered.len(), 2, "should include single + multi, not deduced");

        // Filter: only MultiProvider
        let multi = store.edges_at_quality(EdgeQuality::MultiProvider).await;
        assert_eq!(multi.len(), 1);
        assert_eq!(multi[0].source_node, "zinc");
    }
}
