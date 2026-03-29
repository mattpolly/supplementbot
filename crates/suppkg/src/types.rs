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
