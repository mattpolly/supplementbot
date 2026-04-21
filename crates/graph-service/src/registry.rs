use serde::{Deserialize, Serialize};
use surrealdb::engine::any::Any;
use surrealdb::Surreal;
use surrealdb_types::SurrealValue;

/// A record in the ingredient registry — the canonical multi-source identity
/// for a dietary supplement ingredient.
///
/// Stores cross-references to external databases (SuppKG, iDISK, CTD, UMLS)
/// and search terms used for sentence-based citation mining.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct IngredientRecord {
    /// Primary key: lowercase ingredient name as used in our graph
    pub name: String,
    /// Synonyms and common names (for display and search)
    pub synonyms: Vec<String>,
    /// Terms to grep for in SuppKG sentences (curated, may differ from synonyms)
    pub search_terms: Vec<String>,
    /// Current UMLS CUI (from UMLS API / supplement_cuis.jsonl)
    pub umls_cui: String,
    /// iDISK 2.0 identifier (e.g., "DSI004693")
    pub idisk_id: String,
    /// iDISK's UMLS CUI (may differ from umls_cui due to version)
    pub idisk_cui: String,
    /// CTD MeSH identifier (e.g., "D008274" or "C030693")
    pub ctd_mesh: String,
    /// SuppKG CUI if resolvable (hardcoded or term-matched), empty otherwise
    pub suppkg_cui: String,
}

#[derive(Debug, Clone, SurrealValue)]
struct IngredientRecordWithId {
    #[allow(dead_code)]
    id: surrealdb_types::RecordId,
    pub name: String,
    pub synonyms: Vec<String>,
    pub search_terms: Vec<String>,
    pub umls_cui: String,
    pub idisk_id: String,
    pub idisk_cui: String,
    pub ctd_mesh: String,
    pub suppkg_cui: String,
}

impl From<IngredientRecordWithId> for IngredientRecord {
    fn from(r: IngredientRecordWithId) -> Self {
        Self {
            name: r.name,
            synonyms: r.synonyms,
            search_terms: r.search_terms,
            umls_cui: r.umls_cui,
            idisk_id: r.idisk_id,
            idisk_cui: r.idisk_cui,
            ctd_mesh: r.ctd_mesh,
            suppkg_cui: r.suppkg_cui,
        }
    }
}

/// Manages the ingredient registry — the canonical multi-source identity store
/// for dietary supplement ingredients.
pub struct IngredientRegistry {
    db: Surreal<Any>,
}

impl IngredientRegistry {
    /// Create a registry using the same DB handle as the KnowledgeGraph.
    pub fn new(db: &Surreal<Any>) -> Self {
        Self { db: db.clone() }
    }

    /// Upsert an ingredient record. If the ingredient already exists (by name),
    /// updates all fields. Otherwise inserts a new record.
    pub async fn upsert(&self, record: &IngredientRecord) {
        let name = record.name.to_lowercase();
        let _: surrealdb::Result<Vec<IngredientRecordWithId>> = self
            .db
            .query(
                "UPSERT ingredient_registry SET \
                    name = $name, \
                    synonyms = $synonyms, \
                    search_terms = $search_terms, \
                    umls_cui = $umls_cui, \
                    idisk_id = $idisk_id, \
                    idisk_cui = $idisk_cui, \
                    ctd_mesh = $ctd_mesh, \
                    suppkg_cui = $suppkg_cui \
                 WHERE name = $name",
            )
            .bind(("name", name))
            .bind(("synonyms", record.synonyms.clone()))
            .bind(("search_terms", record.search_terms.clone()))
            .bind(("umls_cui", record.umls_cui.clone()))
            .bind(("idisk_id", record.idisk_id.clone()))
            .bind(("idisk_cui", record.idisk_cui.clone()))
            .bind(("ctd_mesh", record.ctd_mesh.clone()))
            .bind(("suppkg_cui", record.suppkg_cui.clone()))
            .await
            .and_then(|mut r| r.take(0));
    }

    /// Look up an ingredient by name (exact, case-insensitive).
    pub async fn get(&self, name: &str) -> Option<IngredientRecord> {
        let results: Vec<IngredientRecordWithId> = self
            .db
            .query("SELECT * FROM ingredient_registry WHERE name = $name")
            .bind(("name", name.to_lowercase()))
            .await
            .ok()?
            .take(0)
            .unwrap_or_default();

        results.into_iter().next().map(IngredientRecord::from)
    }

    /// Get search terms for an ingredient. Returns the curated search_terms
    /// if populated, otherwise falls back to [name] + synonyms.
    pub async fn search_terms_for(&self, name: &str) -> Vec<String> {
        if let Some(record) = self.get(name).await {
            if !record.search_terms.is_empty() {
                return record.search_terms;
            }
            // Fallback: name + synonyms
            let mut terms = vec![record.name];
            terms.extend(record.synonyms);
            terms
        } else {
            // No registry entry — just use the name itself
            vec![name.to_lowercase()]
        }
    }

    /// List all registered ingredients.
    pub async fn list_all(&self) -> Vec<IngredientRecord> {
        let results: Vec<IngredientRecordWithId> = self
            .db
            .query("SELECT * FROM ingredient_registry ORDER BY name ASC")
            .await
            .unwrap_or_else(|_| unreachable!())
            .take(0)
            .unwrap_or_default();

        results.into_iter().map(IngredientRecord::from).collect()
    }

    /// How many ingredients are registered.
    pub async fn count(&self) -> usize {
        #[derive(SurrealValue)]
        struct CountResult {
            count: u64,
        }
        let result: Vec<CountResult> = self
            .db
            .query("SELECT count() AS count FROM ingredient_registry GROUP ALL")
            .await
            .unwrap_or_else(|_| unreachable!())
            .take(0)
            .unwrap_or_default();

        result.first().map(|r| r.count as usize).unwrap_or(0)
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
    async fn test_upsert_and_get() {
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        let registry = IngredientRegistry::new(kg.db());

        let record = IngredientRecord {
            name: "ashwagandha".to_string(),
            synonyms: vec!["withania somnifera".to_string(), "indian ginseng".to_string()],
            search_terms: vec!["ashwagandha".to_string(), "withania".to_string()],
            umls_cui: "C0613707".to_string(),
            idisk_id: "DSI004693".to_string(),
            idisk_cui: "C0613707".to_string(),
            ctd_mesh: "C030693".to_string(),
            suppkg_cui: String::new(),
        };

        registry.upsert(&record).await;
        let fetched = registry.get("ashwagandha").await.unwrap();
        assert_eq!(fetched.umls_cui, "C0613707");
        assert_eq!(fetched.ctd_mesh, "C030693");
        assert_eq!(fetched.synonyms.len(), 2);
    }

    #[tokio::test]
    async fn test_upsert_updates_existing() {
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        let registry = IngredientRegistry::new(kg.db());

        let mut record = IngredientRecord {
            name: "zinc".to_string(),
            synonyms: vec![],
            search_terms: vec!["zinc".to_string()],
            umls_cui: "C3714650".to_string(),
            idisk_id: String::new(),
            idisk_cui: String::new(),
            ctd_mesh: String::new(),
            suppkg_cui: String::new(),
        };

        registry.upsert(&record).await;
        assert_eq!(registry.count().await, 1);

        // Update with CTD mesh
        record.ctd_mesh = "D015032".to_string();
        registry.upsert(&record).await;

        // Should still be 1 record, not 2
        assert_eq!(registry.count().await, 1);
        let fetched = registry.get("zinc").await.unwrap();
        assert_eq!(fetched.ctd_mesh, "D015032");
    }

    #[tokio::test]
    async fn test_search_terms_fallback() {
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        let registry = IngredientRegistry::new(kg.db());

        // With explicit search_terms
        let record = IngredientRecord {
            name: "coq10".to_string(),
            synonyms: vec!["ubiquinone".to_string(), "coenzyme q10".to_string()],
            search_terms: vec!["coq10".to_string(), "coenzyme q".to_string(), "ubiquinone".to_string()],
            umls_cui: String::new(),
            idisk_id: String::new(),
            idisk_cui: String::new(),
            ctd_mesh: String::new(),
            suppkg_cui: String::new(),
        };
        registry.upsert(&record).await;

        let terms = registry.search_terms_for("coq10").await;
        assert_eq!(terms, vec!["coq10", "coenzyme q", "ubiquinone"]);

        // Without search_terms — falls back to name + synonyms
        let record2 = IngredientRecord {
            name: "iron".to_string(),
            synonyms: vec!["ferrous".to_string()],
            search_terms: vec![],
            umls_cui: String::new(),
            idisk_id: String::new(),
            idisk_cui: String::new(),
            ctd_mesh: String::new(),
            suppkg_cui: String::new(),
        };
        registry.upsert(&record2).await;

        let terms2 = registry.search_terms_for("iron").await;
        assert_eq!(terms2, vec!["iron", "ferrous"]);
    }

    #[tokio::test]
    async fn test_get_missing_returns_none() {
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        let registry = IngredientRegistry::new(kg.db());

        assert!(registry.get("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn test_case_insensitive() {
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        let registry = IngredientRegistry::new(kg.db());

        let record = IngredientRecord {
            name: "Vitamin D".to_string(),
            synonyms: vec![],
            search_terms: vec!["vitamin d".to_string()],
            umls_cui: "C0042866".to_string(),
            idisk_id: String::new(),
            idisk_cui: String::new(),
            ctd_mesh: String::new(),
            suppkg_cui: String::new(),
        };
        registry.upsert(&record).await;

        // Stored as lowercase, queried as mixed case
        let fetched = registry.get("vitamin d").await.unwrap();
        assert_eq!(fetched.umls_cui, "C0042866");
    }

    #[tokio::test]
    async fn test_list_all() {
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        let registry = IngredientRegistry::new(kg.db());

        for name in ["zinc", "iron", "calcium"] {
            registry
                .upsert(&IngredientRecord {
                    name: name.to_string(),
                    synonyms: vec![],
                    search_terms: vec![name.to_string()],
                    umls_cui: String::new(),
                    idisk_id: String::new(),
                    idisk_cui: String::new(),
                    ctd_mesh: String::new(),
                    suppkg_cui: String::new(),
                })
                .await;
        }

        let all = registry.list_all().await;
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].name, "calcium"); // alphabetical
    }
}
