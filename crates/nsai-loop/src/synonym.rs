use std::collections::HashMap;
use std::path::Path;

use event_log::events::PipelineEvent;
use event_log::sink::EventSink;
use graph_service::graph::KnowledgeGraph;
use graph_service::merge::MergeStore;
use graph_service::types::NodeType;
use suppkg::SuppKg;
use umls_client::{append_cache, load_cache, resolve_via_api, SupplementCui};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Synonym resolution — match graph nodes to SuppKG CUIs, detect aliases
//
// This runs after gap-filling and before forward chaining.
// The ordering matters: synonyms must be resolved before inference
// so that forward chaining and structural observations operate on
// a clean graph without duplicates.
// ---------------------------------------------------------------------------

/// Result of a synonym resolution pass
#[derive(Debug, Clone)]
pub struct SynonymResult {
    /// How many nodes were matched to CUIs
    pub cuis_assigned: usize,
    /// How many alias pairs were detected (same CUI, different node name)
    pub aliases_found: usize,
    /// How many CUIs were resolved via UMLS API (vs SuppKG term index)
    pub umls_api_calls: usize,
}

/// Run synonym resolution: match graph nodes to SuppKG CUIs, detect and
/// record aliases for nodes that share a CUI.
///
/// Algorithm:
/// 1. For each node in the graph, try to resolve its name to a CUI via SuppKG
/// 2. If SuppKG fails and the node is an Ingredient, try the UMLS API
/// 3. Record successful CUI mappings in the merge store + supplement_cuis.jsonl
/// 4. When two graph nodes share the same CUI, record an alias
///
/// `umls_api_key` and `cache_path` are optional. When provided, Ingredient
/// nodes that can't be resolved via SuppKG will be looked up via the UMLS API
/// and cached to avoid repeat calls.
pub async fn run_synonym_resolution(
    graph: &KnowledgeGraph,
    suppkg: &SuppKg,
    merge: &MergeStore,
    correlation_id: Uuid,
    sink: &dyn EventSink,
    umls_api_key: Option<&str>,
    cache_path: Option<&Path>,
) -> SynonymResult {
    let mut cuis_assigned = 0;
    let mut aliases_found = 0;
    let mut umls_api_calls = 0;

    // Load UMLS cache if path provided
    let mut cache = cache_path
        .map(|p| load_cache(p))
        .unwrap_or_default();

    // Phase 1: Resolve CUIs for all graph nodes
    // Map from CUI → [(node_name, edge_count)]
    let mut cui_to_nodes: HashMap<String, Vec<(String, usize)>> = HashMap::new();

    for idx in graph.all_nodes().await {
        let data = match graph.node_data(&idx).await {
            Some(d) => d,
            None => continue,
        };

        // Skip if this node already has a CUI — collect for alias detection
        if let Some(cui) = merge.cui_for(&data.name).await {
            let edge_count = graph.outgoing_edges(&idx).await.len()
                + graph.incoming_edges(&idx).await.len();
            cui_to_nodes
                .entry(cui)
                .or_default()
                .push((data.name.clone(), edge_count));
            continue;
        }

        // Try SuppKG exact match first
        if let Some(cui_match) = suppkg.resolve_cui(&data.name) {
            merge
                .record_cui(&data.name, &cui_match.cui, 1.0, "exact_term")
                .await;
            cuis_assigned += 1;
            let edge_count = graph.outgoing_edges(&idx).await.len()
                + graph.incoming_edges(&idx).await.len();
            cui_to_nodes
                .entry(cui_match.cui)
                .or_default()
                .push((data.name.clone(), edge_count));
            continue;
        }

        // For Ingredient nodes: try UMLS API (with cache)
        if data.node_type == NodeType::Ingredient {
            if let Some(rec) = resolve_ingredient_cui(
                &data.name,
                umls_api_key,
                cache_path,
                &mut cache,
                &mut umls_api_calls,
            ).await {
                merge
                    .record_cui(&data.name, &rec.canonical_cui, 0.9, &rec.source)
                    .await;
                cuis_assigned += 1;
                let edge_count = graph.outgoing_edges(&idx).await.len()
                    + graph.incoming_edges(&idx).await.len();
                cui_to_nodes
                    .entry(rec.canonical_cui.clone())
                    .or_default()
                    .push((data.name.clone(), edge_count));
            }
        }
    }

    // Phase 2: Detect aliases — nodes that share a CUI
    for (_cui, nodes) in &cui_to_nodes {
        if nodes.len() < 2 {
            continue;
        }

        // Pick the node with the most edges as canonical
        let mut sorted = nodes.clone();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        let canonical = &sorted[0].0;

        for (alias_name, _) in &sorted[1..] {
            merge
                .record_alias(canonical, alias_name, 1.0, "cui_match")
                .await;
            aliases_found += 1;
        }
    }

    // Emit event summarizing what was found
    sink.emit(
        correlation_id,
        PipelineEvent::SynonymResolution {
            cuis_assigned,
            aliases_found,
        },
    );

    SynonymResult {
        cuis_assigned,
        aliases_found,
        umls_api_calls,
    }
}

// ---------------------------------------------------------------------------
// UMLS resolution helper
// ---------------------------------------------------------------------------

/// Resolve an ingredient name to a UMLS CUI, using the cache first.
/// If not cached and an API key is provided, calls the UMLS API and caches
/// the result. Returns None if no API key or resolution fails.
async fn resolve_ingredient_cui(
    name: &str,
    api_key: Option<&str>,
    cache_path: Option<&Path>,
    cache: &mut HashMap<String, SupplementCui>,
    api_call_count: &mut usize,
) -> Option<SupplementCui> {
    let key = name.to_lowercase();

    // Cache hit
    if let Some(rec) = cache.get(&key) {
        return Some(rec.clone());
    }

    // Need API key to go further
    let api_key = api_key?;

    eprintln!("[umls] resolving '{}' via API...", name);
    let rec = resolve_via_api(name, api_key).await?;
    *api_call_count += 1;

    // Store in cache file
    if let Some(path) = cache_path {
        if let Err(e) = append_cache(path, &rec) {
            eprintln!("[umls] failed to write cache: {}", e);
        }
    }

    cache.insert(key, rec.clone());
    Some(rec)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use event_log::sink::MemorySink;
    use graph_service::types::*;
    use std::io::Cursor;

    fn test_suppkg() -> SuppKg {
        let json = r#"{
            "directed": true,
            "multigraph": true,
            "graph": {},
            "nodes": [
                {"terms": ["magnesium"], "semtypes": ["orch", "phsu"], "id": "C0024467"},
                {"terms": ["nervous system"], "semtypes": ["bdsy"], "id": "C0027763"},
                {"terms": ["muscle relaxation"], "semtypes": ["phsf"], "id": "C0235049"},
                {"terms": ["muscle rest"], "semtypes": ["phsf"], "id": "C0235049"}
            ],
            "links": []
        }"#;
        SuppKg::from_reader(Cursor::new(json)).unwrap()
    }

    #[tokio::test]
    async fn test_assigns_cuis_to_matching_nodes() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let merge = MergeStore::new(graph.db());
        let sink = MemorySink::new();
        let suppkg = test_suppkg();

        graph
            .add_node(NodeData::new("magnesium", NodeType::Ingredient))
            .await;
        graph
            .add_node(NodeData::new("nervous system", NodeType::System))
            .await;

        let corr_id = Uuid::new_v4();
        let result =
            run_synonym_resolution(&graph, &suppkg, &merge, corr_id, &sink, None, None).await;

        assert_eq!(result.cuis_assigned, 2);
        assert_eq!(
            merge.cui_for("magnesium").await,
            Some("C0024467".to_string())
        );
        assert_eq!(
            merge.cui_for("nervous system").await,
            Some("C0027763".to_string())
        );
    }

    #[tokio::test]
    async fn test_detects_aliases_from_shared_cui() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let merge = MergeStore::new(graph.db());
        let sink = MemorySink::new();
        let suppkg = test_suppkg();

        // Both nodes map to C0235049 in our test SuppKG
        let relax = graph
            .add_node(NodeData::new("muscle relaxation", NodeType::Property))
            .await;
        graph
            .add_node(NodeData::new("muscle rest", NodeType::Property))
            .await;

        // Give "muscle relaxation" more edges so it becomes canonical
        let mag = graph
            .add_node(NodeData::new("magnesium", NodeType::Ingredient))
            .await;
        graph
            .add_edge(
                &mag,
                &relax,
                EdgeData::new(EdgeType::Affords, EdgeMetadata::extracted(0.7, 1, 0)),
            )
            .await;

        let corr_id = Uuid::new_v4();
        let result =
            run_synonym_resolution(&graph, &suppkg, &merge, corr_id, &sink, None, None).await;

        assert!(result.aliases_found >= 1, "should detect alias from shared CUI");

        // "muscle rest" should resolve to "muscle relaxation"
        let resolved = merge.resolve("muscle rest").await;
        assert_eq!(resolved, "muscle relaxation");
    }

    #[tokio::test]
    async fn test_skips_nodes_without_cui_match() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let merge = MergeStore::new(graph.db());
        let sink = MemorySink::new();
        let suppkg = test_suppkg();

        graph
            .add_node(NodeData::new("magnesium", NodeType::Ingredient))
            .await;
        graph
            .add_node(NodeData::new("atp synthesis", NodeType::Mechanism))
            .await;

        let corr_id = Uuid::new_v4();
        let result =
            run_synonym_resolution(&graph, &suppkg, &merge, corr_id, &sink, None, None).await;

        // Only magnesium should match; "atp synthesis" isn't in our test SuppKG
        assert_eq!(result.cuis_assigned, 1);
        assert!(merge.cui_for("atp synthesis").await.is_none());
    }

    #[tokio::test]
    async fn test_emits_synonym_event() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let merge = MergeStore::new(graph.db());
        let sink = MemorySink::new();
        let suppkg = test_suppkg();

        graph
            .add_node(NodeData::new("magnesium", NodeType::Ingredient))
            .await;

        let corr_id = Uuid::new_v4();
        run_synonym_resolution(&graph, &suppkg, &merge, corr_id, &sink, None, None).await;

        let events = sink.events_for(corr_id);
        let has_synonym = events
            .iter()
            .any(|e| matches!(e.event, PipelineEvent::SynonymResolution { .. }));
        assert!(has_synonym, "should emit SynonymResolution event");
    }
}
