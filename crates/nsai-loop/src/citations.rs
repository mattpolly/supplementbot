use std::collections::HashMap;

use event_log::events::{CitationRef, PipelineEvent};
use event_log::sink::EventSink;
use graph_service::graph::KnowledgeGraph;
use graph_service::merge::MergeStore;
use graph_service::registry::IngredientRegistry;
use graph_service::source::{CitationRecord, SourceStore};
use graph_service::types::NodeType;
use suppkg::SuppKg;
use uuid::Uuid;

/// Hardcoded CUI overrides for known dietary supplements.
///
/// SuppKG's term index often resolves common supplement names to pharmaceutical
/// excipient or chemical compound CUIs rather than the dietary supplement CUI.
/// For example "magnesium" resolves to DC0126791 (magnesium stearate, 3 edges)
/// rather than C1268858 (magnesium supplement, 118 edges). These overrides use
/// the CUIs with the richest citation coverage for each ingredient.
///
/// Only applied when the override CUI actually has outgoing edges in the loaded
/// SuppKG, so test fixtures with custom CUIs still work.
fn hardcoded_cui(ingredient: &str) -> Option<&'static str> {
    match ingredient.to_lowercase().as_str() {
        "magnesium" => Some("C1268858"),    // magnesium supplement (118 edges vs stearate's 3)
        "zinc" => Some("C1268859"),         // zinc supplement (401 edges, no term match otherwise)
        "vitamin d" | "vitamin d3" => Some("C0535968"), // 25-hydroxyvitamin D (60 edges, best available)
        "vitamin c" => Some("DC0003968"),   // ascorbic acid 6-palmitate (694 edges, only vitamin C node)
        "berberine" => Some("DC0005117"),   // berberina (291 edges vs "berberine"'s 1 edge)
        "omega-3" | "omega 3" | "fish oil" => Some("DC0015689"), // omega-3 essential fatty acids (405 edges)
        _ => None,
        // curcumin: term index gives DC0010467 (384 edges) — correct, no override needed
        // ashwagandha: not in this SuppKG — falls through to sentence search
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
    /// Number of ingredients resolved via CUI (hardcoded/merge/term index)
    pub cui_resolved: usize,
    /// Number of ingredients resolved via sentence search fallback
    pub sentence_resolved: usize,
}

/// For each Ingredient node in the graph, find citations from SuppKG and store them.
///
/// Two resolution strategies:
///
/// 1. **CUI-based** (preferred): Resolve ingredient → CUI via hardcoded overrides,
///    merge store, or SuppKG term index. Then look up all outgoing edges for that CUI.
///    This is precise — it uses SuppKG's graph structure.
///
/// 2. **Sentence search** (fallback): If CUI resolution fails, query the ingredient
///    registry for search terms and scan all SuppKG sentences for mentions. This
///    bypasses the CUI namespace mismatch and recovers citations for ingredients
///    that SuppKG has data on but can't resolve by identifier.
///
/// Citations are stored in `edge_citation` keyed by ingredient name. Deduplication
/// by (source_node, pmid) makes this safe to re-run.
pub async fn run_citation_backing(
    graph: &KnowledgeGraph,
    suppkg: &SuppKg,
    merge_store: &MergeStore,
    source_store: &SourceStore,
    sink: &dyn EventSink,
    correlation_id: Uuid,
) -> CitationBackingResult {
    let registry = IngredientRegistry::new(graph.db());
    run_citation_backing_with_registry(
        graph, suppkg, merge_store, source_store, &registry, sink, correlation_id,
    )
    .await
}

/// Inner implementation that accepts an explicit registry (for testing).
pub async fn run_citation_backing_with_registry(
    graph: &KnowledgeGraph,
    suppkg: &SuppKg,
    merge_store: &MergeStore,
    source_store: &SourceStore,
    registry: &IngredientRegistry,
    sink: &dyn EventSink,
    correlation_id: Uuid,
) -> CitationBackingResult {
    let ingredient_nodes = graph.nodes_by_type(&NodeType::Ingredient).await;

    let mut edges_checked = 0;
    let mut edges_backed = 0;
    let mut citations_stored = 0;
    let mut cui_resolved = 0;
    let mut sentence_resolved = 0;
    let mut sample: Vec<CitationRef> = Vec::new();

    // Phase 1: Try CUI-based resolution for each ingredient.
    // Collect ingredients that fail CUI resolution for batch sentence search.
    let mut sentence_search_ingredients: HashMap<String, Vec<String>> = HashMap::new();

    for idx in &ingredient_nodes {
        let node_data = match graph.node_data(idx).await {
            Some(d) => d,
            None => continue,
        };
        let ingredient_name = node_data.name.to_lowercase();

        edges_checked += 1;

        // Strategy 1: CUI-based resolution (precise)
        let cui_citations = try_cui_based(
            &ingredient_name, suppkg, merge_store, source_store, &mut sample,
        ).await;

        if cui_citations > 0 {
            citations_stored += cui_citations;
            edges_backed += 1;
            cui_resolved += 1;
            continue;
        }

        // Collect for batch sentence search
        let search_terms = registry.search_terms_for(&ingredient_name).await;
        if !search_terms.is_empty() {
            sentence_search_ingredients.insert(ingredient_name, search_terms);
        }
    }

    // Phase 2: Single-pass batch sentence search for all remaining ingredients.
    // 5 citations per (ingredient, target_cui) pair for breadth across use cases.
    if !sentence_search_ingredients.is_empty() {
        let batch_results =
            suppkg.search_sentences_batch(&sentence_search_ingredients, 5);

        for (ingredient_name, matches) in &batch_results {
            let stored = store_sentence_matches(
                ingredient_name, matches, suppkg, source_store, &mut sample,
            ).await;
            if stored > 0 {
                citations_stored += stored;
                edges_backed += 1;
                sentence_resolved += 1;
            }
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
        cui_resolved,
        sentence_resolved,
    }
}

/// Try to resolve an ingredient via CUI and store citations from SuppKG edges.
/// Returns the number of citations stored (0 if CUI resolution failed).
async fn try_cui_based(
    ingredient_name: &str,
    suppkg: &SuppKg,
    merge_store: &MergeStore,
    source_store: &SourceStore,
    sample: &mut Vec<CitationRef>,
) -> usize {
    // CUI resolution priority: hardcoded override → merge store → term index
    let ingredient_cui = if let Some(override_cui) = hardcoded_cui(ingredient_name) {
        if !suppkg.outgoing_edges(override_cui).is_empty() {
            override_cui.to_string()
        } else if let Some(c) = merge_store.cui_for(ingredient_name).await {
            c
        } else if let Some(m) = suppkg.resolve_cui(ingredient_name) {
            m.cui
        } else {
            return 0;
        }
    } else if let Some(c) = merge_store.cui_for(ingredient_name).await {
        c
    } else if let Some(m) = suppkg.resolve_cui(ingredient_name) {
        m.cui
    } else {
        return 0;
    };

    let outgoing = suppkg.outgoing_edges(&ingredient_cui);
    if outgoing.is_empty() {
        return 0;
    }

    let mut stored = 0;

    for (target_cui, predicate) in outgoing {
        let citations = suppkg.citations_for(&ingredient_cui, target_cui, None);
        let target_term = suppkg.first_term_for(target_cui).to_string();

        for citation in citations {
            let record = CitationRecord {
                source_node: ingredient_name.to_string(),
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
                    source_node: ingredient_name.to_string(),
                    target_node: target_term.clone(),
                    edge_type: predicate.to_string(),
                    pmid: citation.pmid.to_string(),
                    suppkg_predicate: predicate.to_string(),
                });
            }
            if source_store.record_citation(&record).await {
                stored += 1;
            }
        }
    }

    stored
}

/// Store sentence search matches for a single ingredient.
/// Returns the number of citations stored.
async fn store_sentence_matches(
    ingredient_name: &str,
    matches: &[suppkg::SentenceMatch],
    suppkg: &SuppKg,
    source_store: &SourceStore,
    sample: &mut Vec<CitationRef>,
) -> usize {
    let mut stored = 0;

    for m in matches {
        let target_term = suppkg.first_term_for(&m.target_cui).to_string();
        let record = CitationRecord {
            source_node: ingredient_name.to_string(),
            target_node: target_term.clone(),
            edge_type: m.predicate.clone(),
            pmid: m.pmid.to_string(),
            sentence: m.sentence.clone(),
            confidence: m.confidence,
            suppkg_predicate: m.predicate.clone(),
            source_cui: m.source_cui.clone(),
            target_cui: m.target_cui.clone(),
        };
        if sample.len() < 20 {
            sample.push(CitationRef {
                source_node: ingredient_name.to_string(),
                target_node: target_term.clone(),
                edge_type: m.predicate.clone(),
                pmid: m.pmid.to_string(),
                suppkg_predicate: m.predicate.clone(),
            });
        }
        if source_store.record_citation(&record).await {
            stored += 1;
        }
    }

    stored
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
        let registry = IngredientRegistry::new(kg.db());
        let sink = MemorySink::new();

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

        kg.add_node(NodeData::new("boron".to_string(), NodeType::Ingredient)).await;
        kg.add_node(NodeData::new("muscular system".to_string(), NodeType::System)).await;

        merge_store
            .record_cui("boron", "C0024467", 1.0, "suppkg_exact")
            .await;

        let result = run_citation_backing_with_registry(
            &kg, &suppkg, &merge_store, &source_store, &registry, &sink, Uuid::new_v4(),
        ).await;

        assert_eq!(result.edges_checked, 1);
        assert_eq!(result.edges_backed, 1);
        assert!(result.citations_stored >= 1);
        assert_eq!(result.cui_resolved, 1);

        let citations = source_store.citations_for_ingredient("boron").await;
        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].pmid, "12345678");
    }

    #[tokio::test]
    async fn test_citation_backing_no_cui_no_match() {
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        let source_store = SourceStore::new(kg.db());
        let merge_store = MergeStore::new(kg.db());
        let registry = IngredientRegistry::new(kg.db());
        let sink = MemorySink::new();

        let json = r#"{
            "directed": true, "multigraph": false, "graph": {},
            "nodes": [{"id": "C9999999", "terms": ["unknown herb"], "semtypes": ["T123"]}],
            "links": []
        }"#;
        let suppkg = SuppKg::from_reader(json.as_bytes()).unwrap();

        kg.add_node(NodeData::new("unknown herb".to_string(), NodeType::Ingredient)).await;

        let result = run_citation_backing_with_registry(
            &kg, &suppkg, &merge_store, &source_store, &registry, &sink, Uuid::new_v4(),
        ).await;

        assert_eq!(result.edges_checked, 1);
        assert_eq!(result.edges_backed, 0);
        assert_eq!(result.citations_stored, 0);
    }

    #[tokio::test]
    async fn test_citation_backing_uses_hardcoded_cui() {
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        let source_store = SourceStore::new(kg.db());
        let merge_store = MergeStore::new(kg.db());
        let registry = IngredientRegistry::new(kg.db());
        let sink = MemorySink::new();

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

        let result = run_citation_backing_with_registry(
            &kg, &suppkg, &merge_store, &source_store, &registry, &sink, Uuid::new_v4(),
        ).await;

        assert_eq!(result.edges_backed, 1);
        assert!(result.citations_stored >= 1);
        assert_eq!(result.cui_resolved, 1);
        let citations = source_store.citations_for_ingredient("magnesium").await;
        assert_eq!(citations[0].source_cui, "C1268858");
    }

    #[tokio::test]
    async fn test_citation_backing_sentence_search_fallback() {
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        let source_store = SourceStore::new(kg.db());
        let merge_store = MergeStore::new(kg.db());
        let registry = IngredientRegistry::new(kg.db());
        let sink = MemorySink::new();

        // SuppKG where "ashwagandha" has no node but appears in a sentence
        let json = r#"{
            "directed": true, "multigraph": false, "graph": {},
            "nodes": [
                {"id": "C0001734", "terms": ["some compound"], "semtypes": ["T123"]},
                {"id": "C0026858", "terms": ["muscular system"], "semtypes": ["T022"]}
            ],
            "links": [
                {
                    "source": "C0001734",
                    "target": "C0026858",
                    "key": "AFFECTS",
                    "relations": [
                        {"pmid": 55555, "sentence": "Ashwagandha supplementation affects muscular recovery.", "conf": 0.88}
                    ]
                }
            ]
        }"#;
        let suppkg = SuppKg::from_reader(json.as_bytes()).unwrap();

        kg.add_node(NodeData::new("ashwagandha".to_string(), NodeType::Ingredient)).await;

        // Register ashwagandha with search terms
        registry
            .upsert(&graph_service::registry::IngredientRecord {
                name: "ashwagandha".to_string(),
                synonyms: vec!["withania somnifera".to_string()],
                search_terms: vec!["ashwagandha".to_string(), "withania".to_string()],
                umls_cui: "C0613707".to_string(),
                idisk_id: String::new(),
                idisk_cui: String::new(),
                ctd_mesh: String::new(),
                suppkg_cui: String::new(),
            })
            .await;

        let result = run_citation_backing_with_registry(
            &kg, &suppkg, &merge_store, &source_store, &registry, &sink, Uuid::new_v4(),
        ).await;

        assert_eq!(result.edges_checked, 1);
        assert_eq!(result.edges_backed, 1);
        assert_eq!(result.citations_stored, 1);
        assert_eq!(result.cui_resolved, 0);
        assert_eq!(result.sentence_resolved, 1);

        let citations = source_store.citations_for_ingredient("ashwagandha").await;
        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].pmid, "55555");
        assert!(citations[0].sentence.contains("Ashwagandha"));
    }

    #[tokio::test]
    async fn test_citation_backing_dedup_on_rerun() {
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        let source_store = SourceStore::new(kg.db());
        let merge_store = MergeStore::new(kg.db());
        let registry = IngredientRegistry::new(kg.db());
        let sink = MemorySink::new();

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

        // Run twice
        let r1 = run_citation_backing_with_registry(
            &kg, &suppkg, &merge_store, &source_store, &registry, &sink, Uuid::new_v4(),
        ).await;
        let r2 = run_citation_backing_with_registry(
            &kg, &suppkg, &merge_store, &source_store, &registry, &sink, Uuid::new_v4(),
        ).await;

        assert_eq!(r1.citations_stored, 1);
        assert_eq!(r2.citations_stored, 0); // Dedup: nothing new stored

        let citations = source_store.citations_for_ingredient("magnesium").await;
        assert_eq!(citations.len(), 1); // Still just 1
    }
}
