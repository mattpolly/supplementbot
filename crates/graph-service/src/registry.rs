use serde::{Deserialize, Serialize};

use crate::client::SupplementClient;

// ---------------------------------------------------------------------------
// IngredientRegistry — canonical multi-source identity for supplement ingredients.
//
// Backed by supplementology Postgres:
//   entity       — canonical name + slug
//   synonym      — synonyms, common names, search terms
//   external_id  — cross-references to iDISK, CTD, UMLS, SuppKG
// ---------------------------------------------------------------------------

/// A record in the ingredient registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngredientRecord {
    pub name: String,
    pub synonyms: Vec<String>,
    pub search_terms: Vec<String>,
    pub umls_cui: String,
    pub idisk_id: String,
    pub idisk_cui: String,
    pub ctd_mesh: String,
    pub suppkg_cui: String,
}

pub struct IngredientRegistry {
    client: SupplementClient,
}

impl IngredientRegistry {
    pub fn new(client: SupplementClient) -> Self {
        Self { client }
    }

    /// Look up an ingredient by name and return its registry record.
    pub async fn get(&self, name: &str) -> Option<IngredientRecord> {
        #[derive(Deserialize)]
        struct Resp {
            name: Option<String>,
            synonyms: Option<Vec<String>>,
            search_terms: Option<Vec<String>>,
            umls_cui: Option<String>,
            idisk_id: Option<String>,
            idisk_cui: Option<String>,
            ctd_mesh: Option<String>,
            suppkg_cui: Option<String>,
        }

        let slug = name.to_lowercase().replace(' ', "_");
        let resp: Option<Resp> = self.client.get_json_pub(
            self.client.http().get(self.client.url_pub(&format!("/v1/ingredients/{}", slug)))
        ).await;

        resp.map(|r| IngredientRecord {
            name: r.name.unwrap_or_else(|| name.to_string()),
            synonyms: r.synonyms.unwrap_or_default(),
            search_terms: r.search_terms.unwrap_or_default(),
            umls_cui: r.umls_cui.unwrap_or_default(),
            idisk_id: r.idisk_id.unwrap_or_default(),
            idisk_cui: r.idisk_cui.unwrap_or_default(),
            ctd_mesh: r.ctd_mesh.unwrap_or_default(),
            suppkg_cui: r.suppkg_cui.unwrap_or_default(),
        })
    }

    /// Get search terms for an ingredient (synonyms + common names used for citation mining).
    pub async fn search_terms_for(&self, name: &str) -> Vec<String> {
        let slug = name.to_lowercase().replace(' ', "_");
        #[derive(Deserialize)]
        struct Resp { query_terms: Vec<String> }

        let resp: Option<Resp> = self.client.get_json_pub(
            self.client.http().get(self.client.url_pub(&format!("/v1/ingredients/{}/query-terms", slug)))
        ).await;

        resp.map(|r| r.query_terms).unwrap_or_else(|| vec![name.to_string()])
    }

    /// List all ingredient names in the registry.
    pub async fn list_all(&self) -> Vec<String> {
        self.client.known_ingredients().await
    }

    /// Total ingredient count.
    pub async fn count(&self) -> usize {
        self.list_all().await.len()
    }
}
