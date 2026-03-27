use serde::{Deserialize, Serialize};
use surrealdb::engine::local::Db;
use surrealdb::Surreal;
use surrealdb_types::SurrealValue;

// ---------------------------------------------------------------------------
// Merge table — non-destructive synonym resolution
//
// Two tables:
//   node_alias  — records that node A is the same concept as node B
//   node_cui    — maps node names to UMLS CUIs
//
// Aliases are soft merges: both nodes stay in the graph, but queries
// resolve through aliases. When confidence is high enough, callers
// can promote to a hard merge (redirect edges, delete duplicate node).
//
// CUI mappings come from SuppKG term matching or future external
// ontology lookups (ChEBI, GO, UMLS).
// ---------------------------------------------------------------------------

/// A recorded alias between two node names
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct AliasRecord {
    /// The canonical node name (the one we keep)
    pub canonical: String,
    /// The alias node name (resolves to canonical)
    pub alias: String,
    /// How confident we are in this alias (0.0–1.0)
    pub confidence: f64,
    /// How this alias was determined
    pub method: String,
    /// When this alias was recorded
    pub created_at: String,
}

/// Returned from select queries (includes SurrealDB's auto-generated id)
#[derive(Debug, Clone, SurrealValue)]
struct AliasRecordWithId {
    #[allow(dead_code)]
    id: surrealdb_types::RecordId,
    pub canonical: String,
    pub alias: String,
    pub confidence: f64,
    pub method: String,
    pub created_at: String,
}

impl From<AliasRecordWithId> for AliasRecord {
    fn from(r: AliasRecordWithId) -> Self {
        Self {
            canonical: r.canonical,
            alias: r.alias,
            confidence: r.confidence,
            method: r.method,
            created_at: r.created_at,
        }
    }
}

/// A CUI mapping for a node
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct CuiRecord {
    /// The node name (lowercase, matches graph node key)
    pub node_name: String,
    /// The UMLS CUI (e.g., "C0024467")
    pub cui: String,
    /// Confidence in this mapping (0.0–1.0)
    pub confidence: f64,
    /// How the CUI was assigned ("exact_term", "fuzzy", "llm")
    pub method: String,
}

/// Returned from select queries
#[derive(Debug, Clone, SurrealValue)]
struct CuiRecordWithId {
    #[allow(dead_code)]
    id: surrealdb_types::RecordId,
    pub node_name: String,
    pub cui: String,
    pub confidence: f64,
    pub method: String,
}

impl From<CuiRecordWithId> for CuiRecord {
    fn from(r: CuiRecordWithId) -> Self {
        Self {
            node_name: r.node_name,
            cui: r.cui,
            confidence: r.confidence,
            method: r.method,
        }
    }
}

// ---------------------------------------------------------------------------
// MergeStore
// ---------------------------------------------------------------------------

/// Manages synonym resolution via alias and CUI tables. Shares a SurrealDB
/// connection with the KnowledgeGraph and SourceStore.
pub struct MergeStore {
    db: Surreal<Db>,
}

impl MergeStore {
    /// Create a merge store using the same DB handle as the KnowledgeGraph.
    pub fn new(db: &Surreal<Db>) -> Self {
        Self { db: db.clone() }
    }

    // -- Alias operations --------------------------------------------------

    /// Record that `alias` is the same concept as `canonical`.
    /// Deduplicates: if the same pair already exists, updates confidence if higher.
    pub async fn record_alias(
        &self,
        canonical: &str,
        alias: &str,
        confidence: f64,
        method: &str,
    ) {
        let canonical = canonical.to_lowercase();
        let alias = alias.to_lowercase();

        // Don't alias a node to itself
        if canonical == alias {
            return;
        }

        // Check if this alias already exists
        let mut result = self
            .db
            .query(
                "SELECT * FROM node_alias WHERE canonical = $can AND alias = $ali",
            )
            .bind(("can", canonical.clone()))
            .bind(("ali", alias.clone()))
            .await
            .unwrap();
        let existing: Vec<AliasRecordWithId> = result.take(0).unwrap_or_default();

        if let Some(existing) = existing.first() {
            // Only update if new confidence is higher
            if confidence > existing.confidence {
                let _: surrealdb::Result<Vec<AliasRecordWithId>> = self
                    .db
                    .query(
                        "UPDATE node_alias SET confidence = $conf, method = $method \
                         WHERE canonical = $can AND alias = $ali",
                    )
                    .bind(("conf", confidence))
                    .bind(("method", method.to_string()))
                    .bind(("can", canonical))
                    .bind(("ali", alias))
                    .await
                    .and_then(|mut r| r.take(0));
            }
        } else {
            let record = AliasRecord {
                canonical,
                alias,
                confidence,
                method: method.to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
            };
            let _: Result<Option<AliasRecord>, _> =
                self.db.create("node_alias").content(record).await;
        }
    }

    /// Resolve a name to its canonical form. Returns the name unchanged if
    /// no alias exists. Single-hop only — no transitive chains.
    pub async fn resolve(&self, name: &str) -> String {
        let name = name.to_lowercase();
        let mut result = self
            .db
            .query("SELECT canonical FROM node_alias WHERE alias = $name")
            .bind(("name", name.clone()))
            .await
            .unwrap();

        #[derive(SurrealValue)]
        struct CanonicalResult {
            canonical: String,
        }

        let records: Vec<CanonicalResult> = result.take(0).unwrap_or_default();
        records
            .into_iter()
            .next()
            .map(|r| r.canonical)
            .unwrap_or(name)
    }

    /// Get all known aliases for a canonical node name.
    pub async fn aliases_for(&self, canonical: &str) -> Vec<AliasRecord> {
        let canonical = canonical.to_lowercase();
        let mut result = self
            .db
            .query("SELECT * FROM node_alias WHERE canonical = $can")
            .bind(("can", canonical))
            .await
            .unwrap();
        let records: Vec<AliasRecordWithId> = result.take(0).unwrap_or_default();
        records.into_iter().map(AliasRecord::from).collect()
    }

    /// Get all alias records in the store.
    pub async fn all_aliases(&self) -> Vec<AliasRecord> {
        let mut result = self
            .db
            .query("SELECT * FROM node_alias")
            .await
            .unwrap();
        let records: Vec<AliasRecordWithId> = result.take(0).unwrap_or_default();
        records.into_iter().map(AliasRecord::from).collect()
    }

    // -- CUI operations ----------------------------------------------------

    /// Record that a node maps to a UMLS CUI.
    /// Deduplicates: if the same node already has a CUI, updates if confidence is higher.
    pub async fn record_cui(
        &self,
        node_name: &str,
        cui: &str,
        confidence: f64,
        method: &str,
    ) {
        let node_name = node_name.to_lowercase();

        // Check if this node already has a CUI
        let mut result = self
            .db
            .query("SELECT * FROM node_cui WHERE node_name = $name")
            .bind(("name", node_name.clone()))
            .await
            .unwrap();
        let existing: Vec<CuiRecordWithId> = result.take(0).unwrap_or_default();

        if let Some(existing) = existing.first() {
            if confidence > existing.confidence {
                let _: surrealdb::Result<Vec<CuiRecordWithId>> = self
                    .db
                    .query(
                        "UPDATE node_cui SET cui = $cui, confidence = $conf, method = $method \
                         WHERE node_name = $name",
                    )
                    .bind(("cui", cui.to_string()))
                    .bind(("conf", confidence))
                    .bind(("method", method.to_string()))
                    .bind(("name", node_name))
                    .await
                    .and_then(|mut r| r.take(0));
            }
        } else {
            let record = CuiRecord {
                node_name,
                cui: cui.to_string(),
                confidence,
                method: method.to_string(),
            };
            let _: Result<Option<CuiRecord>, _> =
                self.db.create("node_cui").content(record).await;
        }
    }

    /// Get the CUI for a node name (resolving through aliases first).
    pub async fn cui_for(&self, name: &str) -> Option<String> {
        let canonical = self.resolve(name).await;
        let mut result = self
            .db
            .query("SELECT cui FROM node_cui WHERE node_name = $name")
            .bind(("name", canonical))
            .await
            .unwrap();

        #[derive(SurrealValue)]
        struct CuiResult {
            cui: String,
        }

        let records: Vec<CuiResult> = result.take(0).unwrap_or_default();
        records.into_iter().next().map(|r| r.cui)
    }

    /// Find all node names that share the same CUI (potential synonyms).
    pub async fn nodes_with_cui(&self, cui: &str) -> Vec<CuiRecord> {
        let mut result = self
            .db
            .query("SELECT * FROM node_cui WHERE cui = $cui")
            .bind(("cui", cui.to_string()))
            .await
            .unwrap();
        let records: Vec<CuiRecordWithId> = result.take(0).unwrap_or_default();
        records.into_iter().map(CuiRecord::from).collect()
    }

    /// Total alias count.
    pub async fn alias_count(&self) -> usize {
        let all = self.all_aliases().await;
        all.len()
    }

    /// Total CUI mapping count.
    pub async fn cui_count(&self) -> usize {
        let mut result = self
            .db
            .query("SELECT count() FROM node_cui GROUP ALL")
            .await
            .unwrap();

        #[derive(SurrealValue)]
        struct CountResult {
            count: usize,
        }

        let counts: Vec<CountResult> = result.take(0).unwrap_or_default();
        counts.into_iter().next().map(|c| c.count).unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::KnowledgeGraph;

    #[tokio::test]
    async fn test_record_and_resolve_alias() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let merge = MergeStore::new(graph.db());

        merge
            .record_alias("muscle relaxation", "muscle rest", 0.95, "cui_match")
            .await;

        let resolved = merge.resolve("muscle rest").await;
        assert_eq!(resolved, "muscle relaxation");
    }

    #[tokio::test]
    async fn test_resolve_unknown_returns_self() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let merge = MergeStore::new(graph.db());

        let resolved = merge.resolve("unknown concept").await;
        assert_eq!(resolved, "unknown concept");
    }

    #[tokio::test]
    async fn test_no_self_alias() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let merge = MergeStore::new(graph.db());

        merge
            .record_alias("magnesium", "magnesium", 1.0, "test")
            .await;

        assert_eq!(merge.alias_count().await, 0);
    }

    #[tokio::test]
    async fn test_alias_deduplication_keeps_higher_confidence() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let merge = MergeStore::new(graph.db());

        merge
            .record_alias("muscle relaxation", "muscle rest", 0.80, "fuzzy")
            .await;
        merge
            .record_alias("muscle relaxation", "muscle rest", 0.95, "cui_match")
            .await;

        let aliases = merge.aliases_for("muscle relaxation").await;
        assert_eq!(aliases.len(), 1);
        assert!((aliases[0].confidence - 0.95).abs() < 0.01);
        assert_eq!(aliases[0].method, "cui_match");
    }

    #[tokio::test]
    async fn test_alias_dedup_ignores_lower_confidence() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let merge = MergeStore::new(graph.db());

        merge
            .record_alias("muscle relaxation", "muscle rest", 0.95, "cui_match")
            .await;
        merge
            .record_alias("muscle relaxation", "muscle rest", 0.80, "fuzzy")
            .await;

        let aliases = merge.aliases_for("muscle relaxation").await;
        assert_eq!(aliases.len(), 1);
        assert!((aliases[0].confidence - 0.95).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_record_and_query_cui() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let merge = MergeStore::new(graph.db());

        merge
            .record_cui("magnesium", "C0024467", 1.0, "exact_term")
            .await;

        let cui = merge.cui_for("magnesium").await;
        assert_eq!(cui, Some("C0024467".to_string()));
    }

    #[tokio::test]
    async fn test_cui_resolves_through_alias() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let merge = MergeStore::new(graph.db());

        merge
            .record_alias("magnesium", "mag", 0.95, "test")
            .await;
        merge
            .record_cui("magnesium", "C0024467", 1.0, "exact_term")
            .await;

        // Query via alias should resolve to the canonical CUI
        let cui = merge.cui_for("mag").await;
        assert_eq!(cui, Some("C0024467".to_string()));
    }

    #[tokio::test]
    async fn test_nodes_with_same_cui() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let merge = MergeStore::new(graph.db());

        merge
            .record_cui("muscle relaxation", "C0235049", 1.0, "exact_term")
            .await;
        merge
            .record_cui("muscle rest", "C0235049", 0.90, "fuzzy")
            .await;

        let nodes = merge.nodes_with_cui("C0235049").await;
        assert_eq!(nodes.len(), 2);
    }

    #[tokio::test]
    async fn test_cui_count() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let merge = MergeStore::new(graph.db());

        merge
            .record_cui("magnesium", "C0024467", 1.0, "exact_term")
            .await;
        merge
            .record_cui("zinc", "C0043481", 1.0, "exact_term")
            .await;

        assert_eq!(merge.cui_count().await, 2);
    }
}
