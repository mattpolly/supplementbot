use event_log::events::{CitationRef, PipelineEvent};
use event_log::sink::EventSink;
use graph_service::graph::KnowledgeGraph;
use graph_service::merge::MergeStore;
use graph_service::source::{CitationRecord, SourceStore};
use graph_service::types::NodeType;
use suppkg::SuppKg;
use uuid::Uuid;

/// Hardcoded CUI overrides for known dietary supplements.
///
/// SuppKG's term index often resolves common supplement names to pharmaceutical
/// excipient or chemical compound CUIs rather than the dietary supplement CUI.
/// For example "magnesium" resolves to magnesium stearate (DC0126791) instead
/// of the dietary magnesium supplement (C1268858). These overrides take
/// precedence over both merge store and SuppKG term resolution.
fn hardcoded_cui(ingredient: &str) -> Option<&'static str> {
    match ingredient.to_lowercase().as_str() {
        "magnesium" => Some("C1268858"),   // magnesium supplement (dietary)
        "zinc" => Some("C1268859"),        // zinc supplement
        "vitamin d" | "vitamin d3" | "cholecalciferol" => Some("C0042866"), // vitamin D
        "vitamin c" | "ascorbic acid" => Some("C0003968"),                  // ascorbic acid
        "berberine" => Some("C0053078"),   // berberine
        "curcumin" | "turmeric" => Some("C0010467"),                        // curcumin
        "omega-3" | "omega 3" | "fish oil" | "epa" | "dha" => Some("C0015347"), // fish oils
        "ashwagandha" | "withania somnifera" => Some("C0600280"),           // withania somnifera
        _ => None,
    }
}

/// Result of running citation backing across the graph.
#[derive(Debug, Clone)]
pub struct CitationBackingResult {
    /// Number of ingredient nodes we checked
    pub edges_checked: usize,
    /// Number of ingredients that got at least one citation
    pub edges_backed: usize,
    /// Total citations stored
    pub citations_stored: usize,
}

/// For each Ingredient node in the graph, find its CUI and store all SuppKG
/// citations for that ingredient.
///
/// This is ingredient-level citation backing: citations are indexed by
/// ingredient name, not by specific edges in our graph. The SuppKG target
/// concepts (body systems, mechanisms, clinical outcomes) are stored as
/// target_node / target_cui so the explore page can display them.
///
/// CUI resolution priority:
/// 1. Hardcoded overrides (corrects known wrong matches in SuppKG term index)
/// 2. Merge store (populated by `--resolve-cuis`)
/// 3. SuppKG term index (fallback)
pub async fn run_citation_backing(
    graph: &KnowledgeGraph,
    suppkg: &SuppKg,
    merge_store: &MergeStore,
    source_store: &SourceStore,
    sink: &dyn EventSink,
    correlation_id: Uuid,
) -> CitationBackingResult {
    let ingredient_nodes = graph.nodes_by_type(&NodeType::Ingredient).await;

    let mut edges_checked = 0;
    let mut edges_backed = 0;
    let mut citations_stored = 0;
    let mut sample: Vec<CitationRef> = Vec::new();

    for idx in &ingredient_nodes {
        let node_data = match graph.node_data(idx).await {
            Some(d) => d,
            None => continue,
        };
        let ingredient_name = node_data.name.to_lowercase();

        edges_checked += 1;

        // Resolve ingredient → CUI.
        // Priority: hardcoded override (if the CUI has edges in SuppKG) →
        //           merge store → SuppKG term index.
        // Hardcoded overrides correct known wrong matches in the real SuppKG term
        // index (e.g. "magnesium" → magnesium stearate instead of dietary magnesium).
        // We only use the override if SuppKG actually has edges for it, so tests
        // with custom SuppKG fixtures can still resolve via the merge store.
        let ingredient_cui = if let Some(override_cui) = hardcoded_cui(&ingredient_name) {
            if !suppkg.outgoing_edges(override_cui).is_empty() {
                override_cui.to_string()
            } else if let Some(c) = merge_store.cui_for(&ingredient_name).await {
                c
            } else if let Some(m) = suppkg.resolve_cui(&ingredient_name) {
                m.cui
            } else {
                continue
            }
        } else if let Some(c) = merge_store.cui_for(&ingredient_name).await {
            c
        } else if let Some(m) = suppkg.resolve_cui(&ingredient_name) {
            m.cui
        } else {
            continue;
        };

        let outgoing = suppkg.outgoing_edges(&ingredient_cui);
        if outgoing.is_empty() {
            continue;
        }

        let mut ingredient_got_citation = false;

        for (target_cui, predicate) in outgoing {
            let citations = suppkg.citations_for(&ingredient_cui, target_cui, None);
            let target_term = suppkg.first_term_for(target_cui).to_string();

            for citation in citations {
                if citation.pmid == 0 {
                    continue;
                }
                let record = CitationRecord {
                    source_node: ingredient_name.clone(),
                    target_node: target_term.clone(),
                    edge_type: predicate.to_string(),
                    pmid: citation.pmid.to_string(),
                    sentence: citation.sentence.clone(),
                    confidence: citation.confidence,
                    suppkg_predicate: predicate.to_string(),
                    source_cui: ingredient_cui.clone(),
                    target_cui: target_cui.to_string(),
                };
                if sample.len() < 20 {
                    sample.push(CitationRef {
                        source_node: ingredient_name.clone(),
                        target_node: target_term.clone(),
                        edge_type: predicate.to_string(),
                        pmid: citation.pmid.to_string(),
                        suppkg_predicate: predicate.to_string(),
                    });
                }
                source_store.record_citation(&record).await;
                citations_stored += 1;
                ingredient_got_citation = true;
            }
        }

        if ingredient_got_citation {
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
            sample,
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
    use graph_service::types::{NodeData, NodeType};

    #[tokio::test]
    async fn test_citation_backing_finds_match_via_merge_store() {
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        let source_store = SourceStore::new(kg.db());
        let merge_store = MergeStore::new(kg.db());
        let sink = MemorySink::new();

        // Use a custom SuppKG with an ingredient that has no hardcoded override
        let json = r#"{
            "directed": true, "multigraph": false, "graph": {},
            "nodes": [
                {"id": "C0024467", "terms": ["boron"], "semtypes": ["T123"]},
                {"id": "C0026858", "terms": ["muscular system", "muscles"], "semtypes": ["T022"]}
            ],
            "links": [
                {
                    "source": "C0024467",
                    "target": "C0026858",
                    "key": "AFFECTS",
                    "relations": [
                        {"pmid": 12345678, "sentence": "Boron affects muscular function.", "conf": 0.85}
                    ]
                }
            ]
        }"#;
        let suppkg = SuppKg::from_reader(json.as_bytes()).unwrap();

        // Add ingredient node (no edges needed — ingredient-level now)
        kg.add_node(NodeData::new("boron".to_string(), NodeType::Ingredient)).await;
        kg.add_node(NodeData::new("muscular system".to_string(), NodeType::System)).await;

        // Map ingredient CUI via merge store
        merge_store
            .record_cui("boron", "C0024467", 1.0, "suppkg_exact")
            .await;

        let result = run_citation_backing(&kg, &suppkg, &merge_store, &source_store, &sink, Uuid::new_v4()).await;

        assert_eq!(result.edges_checked, 1); // 1 ingredient node
        assert_eq!(result.edges_backed, 1);
        assert!(result.citations_stored >= 1);

        // Citation stored with ingredient as source_node
        let citations = source_store.citations_for_ingredient("boron").await;
        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].pmid, "12345678");
        assert_eq!(citations[0].suppkg_predicate, "AFFECTS");
        assert_eq!(citations[0].source_cui, "C0024467");
        assert_eq!(citations[0].target_cui, "C0026858");
    }

    #[tokio::test]
    async fn test_citation_backing_no_cui_no_match() {
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        let source_store = SourceStore::new(kg.db());
        let merge_store = MergeStore::new(kg.db());
        let sink = MemorySink::new();

        // SuppKG with no outgoing edges for the CUI we'll resolve
        let json = r#"{
            "directed": true, "multigraph": false, "graph": {},
            "nodes": [{"id": "C9999999", "terms": ["unknown herb"], "semtypes": ["T123"]}],
            "links": []
        }"#;
        let suppkg = SuppKg::from_reader(json.as_bytes()).unwrap();

        // Ingredient with no CUI in merge store or hardcoded overrides
        kg.add_node(NodeData::new("unknown herb".to_string(), NodeType::Ingredient)).await;
        // SuppKG resolves "unknown herb" → C9999999 but it has no outgoing edges

        let result = run_citation_backing(&kg, &suppkg, &merge_store, &source_store, &sink, Uuid::new_v4()).await;

        assert_eq!(result.edges_checked, 1);
        assert_eq!(result.edges_backed, 0);
        assert_eq!(result.citations_stored, 0);
    }

    #[tokio::test]
    async fn test_citation_backing_uses_hardcoded_cui() {
        // "magnesium" has a hardcoded CUI override; verify it's used even when
        // merge store is empty and SuppKG term index would give the wrong CUI.
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        let source_store = SourceStore::new(kg.db());
        let merge_store = MergeStore::new(kg.db());
        let sink = MemorySink::new();

        // SuppKG with magnesium's hardcoded CUI (C1268858) having an outgoing edge
        let json = r#"{
            "directed": true, "multigraph": false, "graph": {},
            "nodes": [
                {"id": "C1268858", "terms": ["magnesium supplement"], "semtypes": ["T121"]},
                {"id": "C0026858", "terms": ["muscular system"], "semtypes": ["T022"]}
            ],
            "links": [
                {
                    "source": "C1268858",
                    "target": "C0026858",
                    "key": "AFFECTS",
                    "relations": [
                        {"pmid": 99999, "sentence": "Dietary magnesium affects muscles.", "conf": 0.9}
                    ]
                }
            ]
        }"#;
        let suppkg = SuppKg::from_reader(json.as_bytes()).unwrap();

        kg.add_node(NodeData::new("magnesium".to_string(), NodeType::Ingredient)).await;

        let result = run_citation_backing(&kg, &suppkg, &merge_store, &source_store, &sink, Uuid::new_v4()).await;

        assert_eq!(result.edges_backed, 1);
        assert!(result.citations_stored >= 1);
        let citations = source_store.citations_for_ingredient("magnesium").await;
        assert_eq!(citations[0].source_cui, "C1268858");
    }
}
