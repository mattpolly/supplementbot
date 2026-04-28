// ---------------------------------------------------------------------------
// UMLS REST API client + supplement CUI cache
//
// Resolves ingredient names → UMLS CUIs via the NLM UMLS REST API.
// Results are cached in a JSONL file (supplement_cuis.jsonl) so that:
//   - Repeated runs don't burn API quota
//   - Other projects can reuse the resolved CUIs
//   - The full synonym set is preserved for future querying
//
// Cache format: one JSON object per line, keyed by ingredient name (lowercase).
// ---------------------------------------------------------------------------

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A resolved supplement CUI record — one per ingredient, stored in cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupplementCui {
    /// Ingredient name as it appears in our graph (lowercase)
    pub ingredient: String,
    /// The canonical UMLS CUI we chose for this ingredient
    pub canonical_cui: String,
    /// The canonical name returned by UMLS for that CUI
    pub canonical_name: String,
    /// UMLS semantic types (e.g. "Pharmacologic Substance")
    pub semantic_types: Vec<String>,
    /// All English synonyms found for this CUI across UMLS sources
    pub synonyms: Vec<Synonym>,
    /// ISO 8601 timestamp of when this was resolved
    pub resolved_at: String,
    /// How the CUI was found: "umls_exact", "umls_words", "hardcoded"
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Synonym {
    pub name: String,
    pub source: String,
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

/// Load existing supplement CUI cache from a JSONL file.
/// Returns a map from lowercase ingredient name → record.
pub fn load_cache(path: &Path) -> HashMap<String, SupplementCui> {
    let mut map = HashMap::new();
    let Ok(file) = std::fs::File::open(path) else {
        return map;
    };
    for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
        let line = line.trim().to_string();
        if line.is_empty() { continue; }
        if let Ok(rec) = serde_json::from_str::<SupplementCui>(&line) {
            map.insert(rec.ingredient.to_lowercase(), rec);
        }
    }
    map
}

/// Append a single record to the JSONL cache file.
pub fn append_cache(path: &Path, rec: &SupplementCui) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let line = serde_json::to_string(rec).expect("serialize SupplementCui");
    writeln!(file, "{}", line)
}

// ---------------------------------------------------------------------------
// UMLS API client
// ---------------------------------------------------------------------------

/// UMLS REST API response wrappers (internal)
#[derive(Deserialize)]
struct SearchResponse {
    result: SearchResult,
}

#[derive(Deserialize)]
struct SearchResult {
    results: Vec<SearchHit>,
}

#[derive(Deserialize)]
struct SearchHit {
    ui: String,
    name: String,
}

#[derive(Deserialize)]
struct AtomsResponse {
    result: Vec<AtomResult>,
}

#[derive(Deserialize)]
struct AtomResult {
    name: String,
    #[serde(rename = "rootSource")]
    root_source: String,
}

#[derive(Deserialize)]
struct ConceptResponse {
    result: ConceptResult,
}

#[derive(Deserialize)]
struct ConceptResult {
    #[serde(rename = "semanticTypes")]
    semantic_types: Vec<SemanticType>,
}

#[derive(Deserialize)]
struct SemanticType {
    name: String,
}

/// Resolve an ingredient name to a UMLS CUI via the REST API.
///
/// Strategy:
///   1. Exact search
///   2. Word search (picks the first result that looks like a supplement)
///
/// On success, also fetches English synonyms and semantic types for the CUI.
pub async fn resolve_via_api(
    ingredient: &str,
    api_key: &str,
) -> Option<SupplementCui> {
    let client = reqwest::Client::new();

    // Step 1: try exact search
    let hit = search(&client, ingredient, "exact", api_key).await
        .or_else(|| None);

    // Step 2: fall back to word search
    let hit = match hit {
        Some(h) => h,
        None => search(&client, ingredient, "words", api_key).await?,
    };

    let cui = hit.ui.clone();

    // Step 3: fetch semantic types
    let sem_types = fetch_semantic_types(&client, &cui, api_key).await;

    // Step 4: fetch English synonyms (up to 25)
    let synonyms = fetch_synonyms(&client, &cui, api_key).await;

    Some(SupplementCui {
        ingredient: ingredient.to_lowercase(),
        canonical_cui: cui,
        canonical_name: hit.name,
        semantic_types: sem_types,
        synonyms,
        resolved_at: Utc::now().to_rfc3339(),
        source: "umls_api".to_string(),
    })
}

async fn search(
    client: &reqwest::Client,
    term: &str,
    search_type: &str,
    api_key: &str,
) -> Option<SearchHit> {
    let url = format!(
        "https://uts-ws.nlm.nih.gov/rest/search/current\
         ?string={}&apiKey={}&searchType={}&returnIdType=concept&pageSize=5",
        urlencoding::encode(term),
        api_key,
        search_type,
    );

    let resp: SearchResponse = client
        .get(&url)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    // Filter out "NONE" placeholder that UMLS returns when nothing matches
    resp.result.results
        .into_iter()
        .find(|h| h.ui != "NONE")
}

async fn fetch_semantic_types(
    client: &reqwest::Client,
    cui: &str,
    api_key: &str,
) -> Vec<String> {
    let url = format!(
        "https://uts-ws.nlm.nih.gov/rest/content/current/CUI/{}?apiKey={}",
        cui, api_key,
    );

    let resp: ConceptResponse = match client.get(&url).send().await {
        Ok(r) => match r.json().await {
            Ok(j) => j,
            Err(_) => return vec![],
        },
        Err(_) => return vec![],
    };

    resp.result.semantic_types.into_iter().map(|s| s.name).collect()
}

async fn fetch_synonyms(
    client: &reqwest::Client,
    cui: &str,
    api_key: &str,
) -> Vec<Synonym> {
    let url = format!(
        "https://uts-ws.nlm.nih.gov/rest/content/current/CUI/{}/atoms\
         ?apiKey={}&language=ENG&pageSize=25",
        cui, api_key,
    );

    let resp: AtomsResponse = match client.get(&url).send().await {
        Ok(r) => match r.json().await {
            Ok(j) => j,
            Err(_) => return vec![],
        },
        Err(_) => return vec![],
    };

    // Deduplicate by lowercase name
    let mut seen = std::collections::HashSet::new();
    resp.result
        .into_iter()
        .filter(|a| seen.insert(a.name.to_lowercase()))
        .map(|a| Synonym {
            name: a.name,
            source: a.root_source,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// urlencoding helper (avoid adding a dep just for this)
// ---------------------------------------------------------------------------

mod urlencoding {
    pub fn encode(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
                | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
                _ => out.push_str(&format!("%{:02X}", b)),
            }
        }
        out
    }
}
