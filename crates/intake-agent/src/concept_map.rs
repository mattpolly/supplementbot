use graph_service::graph::KnowledgeGraph;
use graph_service::merge::MergeStore;
use graph_service::types::NodeType;

// ---------------------------------------------------------------------------
// Concept mapper — translates free-text user input into graph node names.
//
// Three-tier approach (unanimous consensus):
//   1. Exact/alias match (no LLM) — string match + merge table lookup
//   2. Embedding similarity (no LLM) — future: vector search in SurrealDB
//   3. LLM ranker (fallback) — future: pick from top-5 candidates
//
// For v1, only tier 1 is implemented. Tiers 2-3 will be added when the
// embedding pipeline and LLM integration are ready.
// ---------------------------------------------------------------------------

/// Result of mapping a free-text concept to graph nodes.
#[derive(Debug, Clone)]
pub struct MappingResult {
    /// Matched graph node name (canonical form)
    pub node_name: String,
    /// What type of node was matched
    pub node_type: NodeType,
    /// How the match was found
    pub method: MappingMethod,
}

/// How a concept was mapped to a graph node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MappingMethod {
    /// Direct string match against node name
    ExactMatch,
    /// Resolved through merge table alias
    AliasMatch,
    /// Future: embedding similarity search
    _EmbeddingSimilarity,
    /// Future: LLM-assisted ranking
    _LlmRanker,
}

/// Map free text to graph nodes using exact match + alias resolution.
///
/// Tries to find Symptom nodes first, then System nodes. Returns all matches.
/// The caller can use the node types to decide what's a symptom vs system mapping.
pub async fn map_text_to_nodes(
    text: &str,
    graph: &KnowledgeGraph,
    merge: &MergeStore,
) -> Vec<MappingResult> {
    let mut results = Vec::new();

    // Normalize input
    let normalized = text.to_lowercase().trim().to_string();

    // Try exact match first
    if let Some(idx) = graph.find_node(&normalized).await {
        if let Some(data) = graph.node_data(&idx).await {
            results.push(MappingResult {
                node_name: data.name.to_lowercase(),
                node_type: data.node_type,
                method: MappingMethod::ExactMatch,
            });
            return results;
        }
    }

    // Try alias resolution
    let canonical = merge.resolve(&normalized).await;
    if canonical != normalized {
        if let Some(idx) = graph.find_node(&canonical).await {
            if let Some(data) = graph.node_data(&idx).await {
                results.push(MappingResult {
                    node_name: data.name.to_lowercase(),
                    node_type: data.node_type,
                    method: MappingMethod::AliasMatch,
                });
                return results;
            }
        }
    }

    // Try find_node_or_alias (which does both in one call)
    if let Some(idx) = graph.find_node_or_alias(&normalized, merge).await {
        if let Some(data) = graph.node_data(&idx).await {
            let method = if data.name.to_lowercase() == normalized {
                MappingMethod::ExactMatch
            } else {
                MappingMethod::AliasMatch
            };
            results.push(MappingResult {
                node_name: data.name.to_lowercase(),
                node_type: data.node_type,
                method,
            });
        }
    }

    results
}

/// Map a user's chief complaint text to symptoms and systems.
/// Returns (symptom_names, system_names).
///
/// This is a simple word/phrase extraction for v1 — tries each word and
/// multi-word phrase against the graph. Future versions will use embedding
/// similarity and LLM-assisted extraction.
pub async fn map_complaint(
    raw_text: &str,
    graph: &KnowledgeGraph,
    merge: &MergeStore,
) -> (Vec<String>, Vec<String>) {
    let mut symptoms = Vec::new();
    let mut systems = Vec::new();

    // Try the full text first
    let full_results = map_text_to_nodes(raw_text, graph, merge).await;
    for r in &full_results {
        match r.node_type {
            NodeType::Symptom => symptoms.push(r.node_name.clone()),
            NodeType::System => systems.push(r.node_name.clone()),
            _ => {}
        }
    }

    // If the full text didn't match, try individual words and bigrams
    if full_results.is_empty() {
        let words: Vec<&str> = raw_text.split_whitespace().collect();

        // Try each word
        for word in &words {
            let results = map_text_to_nodes(word, graph, merge).await;
            for r in results {
                match r.node_type {
                    NodeType::Symptom if !symptoms.contains(&r.node_name) => {
                        symptoms.push(r.node_name);
                    }
                    NodeType::System if !systems.contains(&r.node_name) => {
                        systems.push(r.node_name);
                    }
                    _ => {}
                }
            }
        }

        // Try bigrams (two consecutive words)
        for pair in words.windows(2) {
            let bigram = format!("{} {}", pair[0], pair[1]);
            let results = map_text_to_nodes(&bigram, graph, merge).await;
            for r in results {
                match r.node_type {
                    NodeType::Symptom if !symptoms.contains(&r.node_name) => {
                        symptoms.push(r.node_name);
                    }
                    NodeType::System if !systems.contains(&r.node_name) => {
                        systems.push(r.node_name);
                    }
                    _ => {}
                }
            }
        }

        // Try trigrams
        for triple in words.windows(3) {
            let trigram = format!("{} {} {}", triple[0], triple[1], triple[2]);
            let results = map_text_to_nodes(&trigram, graph, merge).await;
            for r in results {
                match r.node_type {
                    NodeType::Symptom if !symptoms.contains(&r.node_name) => {
                        symptoms.push(r.node_name);
                    }
                    NodeType::System if !systems.contains(&r.node_name) => {
                        systems.push(r.node_name);
                    }
                    _ => {}
                }
            }
        }
    }

    (symptoms, systems)
}

/// Log an unmapped concept for later ontology expansion.
/// For v1 this just prints; future versions will persist to a table.
pub fn log_unmapped(raw_text: &str, context: &str) {
    eprintln!(
        "[concept_map] UNMAPPED: \"{}\" (context: {})",
        raw_text, context
    );
}
