use event_log::events::{CitationRef, PipelineEvent};
use event_log::sink::EventSink;
use graph_service::graph::KnowledgeGraph;
use graph_service::merge::MergeStore;
use graph_service::source::{CitationRecord, SourceStore};
use suppkg::SuppKg;
use uuid::Uuid;

/// Maps SuppKG predicates to our graph's edge types.
/// Returns None for predicates we don't want to use (e.g. TREATS — legal risk).
#[allow(dead_code)]
fn suppkg_predicate_to_edge_type(predicate: &str) -> Option<&'static str> {
    match predicate {
        "AFFECTS" => Some("acts_on"),
        "INHIBITS" => Some("modulates"),
        "STIMULATES" => Some("modulates"),
        "PROCESS_OF" => Some("via_mechanism"),
        "INTERACTS_WITH" => Some("modulates"),
        "DISRUPTS" => Some("modulates"),
        "AUGMENTS" => Some("affords"),
        "CAUSES" => Some("affords"),
        "PREDISPOSES" => Some("affords"),
        "ASSOCIATED_WITH" => Some("acts_on"),
        _ => None,
    }
}

/// Maps our graph's edge type strings to compatible SuppKG predicates.
fn edge_type_to_suppkg_predicates(edge_type: &str) -> &'static [&'static str] {
    match edge_type {
        "acts_on" => &["AFFECTS", "ASSOCIATED_WITH"],
        "modulates" => &["INHIBITS", "STIMULATES", "INTERACTS_WITH", "DISRUPTS"],
        "via_mechanism" => &["PROCESS_OF"],
        "affords" => &["AUGMENTS", "CAUSES", "PREDISPOSES"],
        _ => &[],
    }
}

/// Result of running citation backing across the graph.
#[derive(Debug, Clone)]
pub struct CitationBackingResult {
    /// Number of graph edges we tried to match
    pub edges_checked: usize,
    /// Number of graph edges that got at least one citation
    pub edges_backed: usize,
    /// Total citations stored
    pub citations_stored: usize,
}

/// For each edge in the graph, try to find SuppKG citations via CUI resolution.
///
/// Flow per edge:
/// 1. Resolve source node → CUI (via merge store)
/// 2. Resolve target node → CUI (via merge store)
/// 3. Look up SuppKG edges between those CUIs with compatible predicates
/// 4. Store any citations found
pub async fn run_citation_backing(
    graph: &KnowledgeGraph,
    suppkg: &SuppKg,
    merge_store: &MergeStore,
    source_store: &SourceStore,
    sink: &dyn EventSink,
    correlation_id: Uuid,
) -> CitationBackingResult {
    let all_edges = graph.all_edges().await;

    let mut edges_checked = 0;
    let mut edges_backed = 0;
    let mut citations_stored = 0;
    let mut sample: Vec<CitationRef> = Vec::new();

    for (source_idx, target_idx, edge_data) in &all_edges {
        let source_data = graph.node_data(source_idx).await;
        let target_data = graph.node_data(target_idx).await;

        let (source_name, target_name) = match (source_data, target_data) {
            (Some(s), Some(t)) => (s.name, t.name),
            _ => continue,
        };

        let edge_type = edge_data.edge_type.to_string();
        let compatible_predicates = edge_type_to_suppkg_predicates(&edge_type);
        if compatible_predicates.is_empty() {
            continue;
        }

        edges_checked += 1;

        // Resolve both nodes to CUIs
        let source_cui = merge_store.cui_for(&source_name).await;
        let target_cui = merge_store.cui_for(&target_name).await;

        let (source_cui, target_cui) = match (source_cui, target_cui) {
            (Some(s), Some(t)) => (s, t),
            _ => continue,
        };

        let mut edge_got_citation = false;

        // Check each compatible predicate
        for predicate in compatible_predicates {
            let citations = suppkg.citations_for(&source_cui, &target_cui, Some(predicate));
            for citation in citations {
                let record = CitationRecord {
                    source_node: source_name.clone(),
                    target_node: target_name.clone(),
                    edge_type: edge_type.clone(),
                    pmid: citation.pmid.to_string(),
                    sentence: citation.sentence.clone(),
                    confidence: citation.confidence,
                    suppkg_predicate: predicate.to_string(),
                    source_cui: source_cui.clone(),
                    target_cui: target_cui.clone(),
                };
                sample.push(CitationRef {
                    source_node: source_name.clone(),
                    target_node: target_name.clone(),
                    edge_type: edge_type.clone(),
                    pmid: citation.pmid.to_string(),
                    suppkg_predicate: predicate.to_string(),
                });
                source_store.record_citation(&record).await;
                citations_stored += 1;
                edge_got_citation = true;
            }
        }

        // Also check reverse direction — SuppKG might have target→source
        for predicate in compatible_predicates {
            let citations = suppkg.citations_for(&target_cui, &source_cui, Some(predicate));
            for citation in citations {
                let record = CitationRecord {
                    source_node: source_name.clone(),
                    target_node: target_name.clone(),
                    edge_type: edge_type.clone(),
                    pmid: citation.pmid.to_string(),
                    sentence: citation.sentence.clone(),
                    confidence: citation.confidence,
                    suppkg_predicate: predicate.to_string(),
                    source_cui: target_cui.clone(),
                    target_cui: source_cui.clone(),
                };
                sample.push(CitationRef {
                    source_node: source_name.clone(),
                    target_node: target_name.clone(),
                    edge_type: edge_type.clone(),
                    pmid: citation.pmid.to_string(),
                    suppkg_predicate: predicate.to_string(),
                });
                source_store.record_citation(&record).await;
                citations_stored += 1;
                edge_got_citation = true;
            }
        }

        if edge_got_citation {
            edges_backed += 1;
        }
    }

    // Emit pipeline event
    sink.emit(
        correlation_id,
        PipelineEvent::CitationBacking {
            edges_checked,
            edges_backed,
            citations_stored,
            sample: sample.into_iter().take(20).collect(),
        },
    );

    CitationBackingResult {
        edges_checked,
        edges_backed,
        citations_stored,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use event_log::sink::MemorySink;
    use graph_service::types::{EdgeData, EdgeMetadata, EdgeType, NodeData, NodeType};

    async fn add_node(kg: &KnowledgeGraph, name: &str, nt: NodeType) {
        kg.add_node(NodeData::new(name.to_string(), nt)).await;
    }

    async fn add_edge(kg: &KnowledgeGraph, src: &str, tgt: &str, et: EdgeType) {
        let s = kg.find_node(src).await.unwrap();
        let t = kg.find_node(tgt).await.unwrap();
        kg.add_edge(
            &s,
            &t,
            EdgeData::new(et, EdgeMetadata::extracted(0.7, 0, 0)),
        )
        .await;
    }

    fn make_suppkg() -> SuppKg {
        let json = r#"{
            "directed": true,
            "multigraph": false,
            "graph": {},
            "nodes": [
                {"id": "C0024467", "terms": ["magnesium"], "semtypes": ["T123"]},
                {"id": "C0026858", "terms": ["muscular system", "muscles"], "semtypes": ["T022"]}
            ],
            "links": [
                {
                    "source": "C0024467",
                    "target": "C0026858",
                    "key": "AFFECTS",
                    "relations": [
                        {"pmid": 12345678, "sentence": "Magnesium affects muscular function.", "conf": 0.85}
                    ]
                }
            ]
        }"#;
        SuppKg::from_reader(json.as_bytes()).unwrap()
    }

    #[tokio::test]
    async fn test_citation_backing_finds_match() {
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        let source_store = SourceStore::new(kg.db());
        let merge_store = MergeStore::new(kg.db());
        let sink = MemorySink::new();
        let suppkg = make_suppkg();

        // Add nodes and edge to our graph
        add_node(&kg, "magnesium", NodeType::Ingredient).await;
        add_node(&kg, "muscular system", NodeType::System).await;
        add_edge(&kg, "magnesium", "muscular system", EdgeType::ActsOn).await;

        // Record CUI mappings (normally done by synonym resolution)
        merge_store
            .record_cui("magnesium", "C0024467", 1.0, "suppkg_exact")
            .await;
        merge_store
            .record_cui("muscular system", "C0026858", 1.0, "suppkg_exact")
            .await;

        let result = run_citation_backing(&kg, &suppkg, &merge_store, &source_store, &sink, Uuid::new_v4()).await;

        assert_eq!(result.edges_checked, 1);
        assert_eq!(result.edges_backed, 1);
        assert!(result.citations_stored >= 1);

        // Verify stored citation
        let citations = source_store
            .citations_for_edge("magnesium", "muscular system", "acts_on")
            .await;
        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].pmid, "12345678");
        assert_eq!(citations[0].suppkg_predicate, "AFFECTS");
        assert_eq!(citations[0].source_cui, "C0024467");
    }

    #[tokio::test]
    async fn test_citation_backing_no_cui_no_match() {
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        let source_store = SourceStore::new(kg.db());
        let merge_store = MergeStore::new(kg.db());
        let sink = MemorySink::new();
        let suppkg = make_suppkg();

        // Add edge but no CUI mappings
        add_node(&kg, "magnesium", NodeType::Ingredient).await;
        add_node(&kg, "muscular system", NodeType::System).await;
        add_edge(&kg, "magnesium", "muscular system", EdgeType::ActsOn).await;

        let result = run_citation_backing(&kg, &suppkg, &merge_store, &source_store, &sink, Uuid::new_v4()).await;

        assert_eq!(result.edges_checked, 1);
        assert_eq!(result.edges_backed, 0);
        assert_eq!(result.citations_stored, 0);
    }

    #[tokio::test]
    async fn test_citation_backing_incompatible_predicate() {
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        let source_store = SourceStore::new(kg.db());
        let merge_store = MergeStore::new(kg.db());
        let sink = MemorySink::new();

        // SuppKG has AFFECTS but our edge is via_mechanism — AFFECTS doesn't map to via_mechanism
        let json = r#"{
            "directed": true, "multigraph": false, "graph": {},
            "nodes": [
                {"id": "C0024467", "terms": ["magnesium"], "semtypes": ["T123"]},
                {"id": "C9999999", "terms": ["atp synthesis"], "semtypes": ["T044"]}
            ],
            "links": [
                {
                    "source": "C0024467",
                    "target": "C9999999",
                    "key": "AFFECTS",
                    "relations": [
                        {"pmid": 99999, "sentence": "Mag affects ATP.", "conf": 0.7}
                    ]
                }
            ]
        }"#;
        let suppkg = SuppKg::from_reader(json.as_bytes()).unwrap();

        add_node(&kg, "magnesium", NodeType::Ingredient).await;
        add_node(&kg, "atp synthesis", NodeType::Mechanism).await;
        add_edge(&kg, "magnesium", "atp synthesis", EdgeType::ViaMechanism).await;

        merge_store.record_cui("magnesium", "C0024467", 1.0, "suppkg_exact").await;
        merge_store.record_cui("atp synthesis", "C9999999", 1.0, "suppkg_exact").await;

        let result = run_citation_backing(&kg, &suppkg, &merge_store, &source_store, &sink, Uuid::new_v4()).await;

        // via_mechanism maps to PROCESS_OF, but SuppKG only has AFFECTS here
        assert_eq!(result.edges_backed, 0);
        assert_eq!(result.citations_stored, 0);
    }

    #[tokio::test]
    async fn test_predicate_mapping() {
        assert_eq!(suppkg_predicate_to_edge_type("AFFECTS"), Some("acts_on"));
        assert_eq!(suppkg_predicate_to_edge_type("STIMULATES"), Some("modulates"));
        assert_eq!(suppkg_predicate_to_edge_type("TREATS"), None);
        assert_eq!(suppkg_predicate_to_edge_type("PROCESS_OF"), Some("via_mechanism"));
    }
}
