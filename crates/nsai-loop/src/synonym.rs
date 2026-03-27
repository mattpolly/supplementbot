use std::collections::HashMap;

use event_log::events::PipelineEvent;
use event_log::sink::EventSink;
use graph_service::graph::KnowledgeGraph;
use graph_service::merge::MergeStore;
use suppkg::SuppKg;
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
}

/// Run synonym resolution: match graph nodes to SuppKG CUIs, detect and
/// record aliases for nodes that share a CUI.
///
/// Algorithm:
/// 1. For each node in the graph, try to resolve its name to a CUI via SuppKG
/// 2. Record successful CUI mappings in the merge store
/// 3. When two graph nodes share the same CUI, record an alias (the node
///    with more edges becomes canonical)
pub async fn run_synonym_resolution(
    graph: &KnowledgeGraph,
    suppkg: &SuppKg,
    merge: &MergeStore,
    correlation_id: Uuid,
    sink: &dyn EventSink,
) -> SynonymResult {
    let mut cuis_assigned = 0;
    let mut aliases_found = 0;

    // Phase 1: Resolve CUIs for all graph nodes
    // Map from CUI → [(node_name, edge_count)]
    let mut cui_to_nodes: HashMap<String, Vec<(String, usize)>> = HashMap::new();

    for idx in graph.all_nodes().await {
        let data = match graph.node_data(&idx).await {
            Some(d) => d,
            None => continue,
        };

        // Skip if this node already has a CUI
        if merge.cui_for(&data.name).await.is_some() {
            // Still collect it for alias detection
            if let Some(cui) = merge.cui_for(&data.name).await {
                let edge_count = graph.outgoing_edges(&idx).await.len()
                    + graph.incoming_edges(&idx).await.len();
                cui_to_nodes
                    .entry(cui)
                    .or_default()
                    .push((data.name.clone(), edge_count));
            }
            continue;
        }

        // Try to resolve via SuppKG
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
    }
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
            run_synonym_resolution(&graph, &suppkg, &merge, corr_id, &sink).await;

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
            run_synonym_resolution(&graph, &suppkg, &merge, corr_id, &sink).await;

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
            run_synonym_resolution(&graph, &suppkg, &merge, corr_id, &sink).await;

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
        run_synonym_resolution(&graph, &suppkg, &merge, corr_id, &sink).await;

        let events = sink.events_for(corr_id);
        let has_synonym = events
            .iter()
            .any(|e| matches!(e.event, PipelineEvent::SynonymResolution { .. }));
        assert!(has_synonym, "should emit SynonymResolution event");
    }
}
