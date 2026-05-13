use serde::{Deserialize, Serialize};

use crate::client::SupplementClient;

// ---------------------------------------------------------------------------
// Merge store — synonym resolution via supplementology Postgres tables.
//
// Two tables (backed by the supplementology API):
//   node_alias  — canonical/alias pairs with confidence and method
//   node_cui    — node-to-UMLS-CUI mappings
//
// Read operations go through the API. Write operations POST to the API.
// The NSAI loop calls record_alias / record_cui to persist new mappings.
// ---------------------------------------------------------------------------

/// A recorded alias between two node names.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliasRecord {
    pub canonical: String,
    pub alias: String,
    pub confidence: f64,
    pub method: String,
    pub created_at: String,
}

/// A CUI mapping for a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuiRecord {
    pub node_name: String,
    pub cui: String,
    pub confidence: f64,
    pub method: String,
}

// ---------------------------------------------------------------------------
// MergeStore
// ---------------------------------------------------------------------------

/// Manages synonym resolution via the supplementology API.
#[derive(Clone)]
pub struct MergeStore {
    client: SupplementClient,
}

impl MergeStore {
    pub fn new(client: SupplementClient) -> Self {
        Self { client }
    }

    // -- Alias operations --------------------------------------------------

    /// Record that `alias` is the same concept as `canonical`.
    pub async fn record_alias(
        &self,
        canonical: &str,
        alias: &str,
        confidence: f64,
        method: &str,
    ) {
        self.client.record_alias(canonical, alias, confidence, method).await;
    }

    /// Resolve a name to its canonical form. Returns the name unchanged if
    /// no alias exists. Single-hop only.
    pub async fn resolve(&self, name: &str) -> String {
        self.client.resolve_alias(name).await
    }

    /// Get all known aliases for a canonical node name.
    pub async fn aliases_for(&self, canonical: &str) -> Vec<AliasRecord> {
        self.client.aliases_for(canonical).await
    }

    /// Get all alias records in the store.
    pub async fn all_aliases(&self) -> Vec<AliasRecord> {
        self.client.all_aliases().await
    }

    // -- CUI operations ----------------------------------------------------

    /// Record that a node maps to a UMLS CUI.
    pub async fn record_cui(
        &self,
        node_name: &str,
        cui: &str,
        confidence: f64,
        method: &str,
    ) {
        self.client.record_cui(node_name, cui, confidence, method).await;
    }

    /// Get the CUI for a node name (resolving through aliases first).
    pub async fn cui_for(&self, name: &str) -> Option<String> {
        self.client.cui_for(name).await
    }

    /// Find all node names that share the same CUI (potential synonyms).
    pub async fn nodes_with_cui(&self, cui: &str) -> Vec<CuiRecord> {
        self.client.nodes_with_cui(cui).await
    }

    /// Get all CUI records.
    pub async fn all_cuis(&self) -> Vec<CuiRecord> {
        self.client.all_cuis().await
    }

    /// Total alias count.
    pub async fn alias_count(&self) -> usize {
        self.all_aliases().await.len()
    }

    /// Total CUI mapping count.
    pub async fn cui_count(&self) -> usize {
        self.all_cuis().await.len()
    }
}
