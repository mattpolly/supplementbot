use serde::Deserialize;

/// A node in SuppKG — a UMLS concept with terms and semantic types
#[derive(Debug, Clone)]
pub struct SuppNode {
    pub cui: String,
    pub terms: Vec<String>,
    pub semtypes: Vec<String>,
}

/// An edge in SuppKG — a predicate between two CUIs with PubMed citations
#[derive(Debug, Clone)]
pub struct SuppEdge {
    pub source_cui: String,
    pub target_cui: String,
    pub predicate: String,
    pub citations: Vec<Citation>,
}

/// A single PubMed citation backing an edge
#[derive(Debug, Clone)]
pub struct Citation {
    pub pmid: u64,
    pub sentence: String,
    pub confidence: f64,
}

/// A citation found by searching sentence text for ingredient names.
#[derive(Debug, Clone)]
pub struct SentenceMatch {
    /// The CUI of the source node in SuppKG
    pub source_cui: String,
    /// The CUI of the target node in SuppKG
    pub target_cui: String,
    /// The SuppKG predicate (e.g., AFFECTS, STIMULATES)
    pub predicate: String,
    /// PubMed ID (0 if from v2 edgelist which lacks PMIDs)
    pub pmid: u64,
    /// The supporting sentence that matched
    pub sentence: String,
    /// SuppKG's confidence score (0.0 if from v2 edgelist)
    pub confidence: f64,
    /// Which search term triggered the match
    pub matched_term: String,
}

/// Result of resolving a node name to a CUI
#[derive(Debug, Clone)]
pub struct CuiMatch {
    pub cui: String,
    pub matched_term: String,
    pub terms: Vec<String>,
    pub semtypes: Vec<String>,
}

// -- Serde structs for the NetworkX JSON format ----------------------------

#[derive(Deserialize)]
pub(crate) struct NxGraph {
    pub nodes: Vec<NxNode>,
    pub links: Vec<NxLink>,
}

#[derive(Deserialize)]
pub(crate) struct NxNode {
    pub id: String,
    #[serde(default)]
    pub terms: Vec<String>,
    #[serde(default)]
    pub semtypes: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct NxLink {
    pub source: String,
    pub target: String,
    pub key: String,
    #[serde(default)]
    pub relations: Vec<NxRelation>,
}

#[derive(Deserialize)]
pub(crate) struct NxRelation {
    #[serde(default)]
    pub pmid: u64,
    #[serde(default)]
    pub sentence: String,
    #[serde(default)]
    pub conf: f64,
}
