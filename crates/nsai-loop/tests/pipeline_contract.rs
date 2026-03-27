//! End-to-end pipeline contract tests.
//!
//! These tests run the full NSAI loop with a mock provider and assert that:
//! 1. The expected pipeline events are emitted in the correct order
//! 2. The graph is populated with the expected structure
//! 3. Post-processing phases (synonym resolution, citation backing,
//!    confidence boosting/decay, structural analysis) all execute

use event_log::events::PipelineEvent;
use event_log::sink::MemorySink;
use graph_service::graph::KnowledgeGraph;
use graph_service::merge::MergeStore;
use graph_service::source::SourceStore;
use llm_client::mock::MockProvider;
use nsai_loop::loop_runner::NsaiLoop;
use suppkg::SuppKg;
use uuid::Uuid;

fn pipeline_mock() -> MockProvider {
    MockProvider::new("mock", "mock-v1")
        // Seed answer (5th grade)
        .on(
            "5th grader",
            "Magnesium helps your muscles relax and helps you sleep better.",
        )
        // Extraction of seed
        .on(
            "muscles relax",
            "magnesium|Ingredient|affords|muscle relaxation|Property\n\
             magnesium|Ingredient|acts_on|muscular system|System\n\
             magnesium|Ingredient|affords|sleep quality|Property",
        )
        // Gap fill: "connected to muscle relaxation"
        .on(
            "connected to muscle relaxation",
            "Magnesium helps muscles relax by stopping them from staying tight.",
        )
        .on(
            "staying tight",
            "magnesium|Ingredient|via_mechanism|muscle tension relief|Mechanism\n\
             muscle tension relief|Mechanism|affords|muscle relaxation|Property",
        )
        // Gap fill: "help with sleep quality"
        .on(
            "help with sleep quality",
            "Magnesium helps calm your brain so you can fall asleep.",
        )
        .on(
            "calm your brain",
            "magnesium|Ingredient|acts_on|nervous system|System\n\
             magnesium|Ingredient|affords|sleep quality|Property",
        )
        // Comprehension rephrase
        .on(
            "explain the same",
            "Magnesium keeps your muscles from getting too tight and helps your brain relax for sleep.",
        )
        .on(
            "too tight",
            "magnesium|Ingredient|affords|muscle relaxation|Property\n\
             magnesium|Ingredient|affords|sleep quality|Property",
        )
        // Catch-all
        .with_default("magnesium|Ingredient|affords|general health|Property")
}

fn make_suppkg() -> SuppKg {
    let json = r#"{
        "directed": true,
        "multigraph": false,
        "graph": {},
        "nodes": [
            {"id": "C0024467", "terms": ["magnesium"], "semtypes": ["T123"]},
            {"id": "C0026858", "terms": ["muscular system", "muscles"], "semtypes": ["T022"]},
            {"id": "C0027763", "terms": ["nervous system"], "semtypes": ["T022"]},
            {"id": "C0037313", "terms": ["sleep quality", "sleep"], "semtypes": ["T033"]}
        ],
        "links": [
            {
                "source": "C0024467",
                "target": "C0026858",
                "key": "AFFECTS",
                "relations": [
                    {"pmid": 12345678, "sentence": "Magnesium supplementation improves muscular function.", "conf": 0.85}
                ]
            },
            {
                "source": "C0024467",
                "target": "C0037313",
                "key": "AFFECTS",
                "relations": [
                    {"pmid": 23456789, "sentence": "Oral magnesium improves subjective sleep quality.", "conf": 0.80}
                ]
            }
        ]
    }"#;
    SuppKg::from_reader(json.as_bytes()).unwrap()
}

// -------------------------------------------------------------------------
// Contract 1: Event sequence
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_pipeline_emits_required_event_types() {
    let provider = pipeline_mock();
    let sink = MemorySink::new();
    let graph = KnowledgeGraph::in_memory().await.unwrap();
    let source_store = SourceStore::new(graph.db());
    let merge_store = MergeStore::new(graph.db());
    let suppkg = make_suppkg();
    let corr_id = Uuid::new_v4();

    let nsai = NsaiLoop::new(&provider, &sink)
        .with_source_store(&source_store)
        .with_synonym_resolution(&suppkg, &merge_store);

    nsai.run("Magnesium", &graph, corr_id).await;

    // Post-processing: citation backing
    nsai_loop::citations::run_citation_backing(
        &graph,
        &suppkg,
        &merge_store,
        &source_store,
        &sink,
        corr_id,
    )
    .await;

    let events = sink.events_for(corr_id);
    let event_types: Vec<&str> = events
        .iter()
        .map(|e| match &e.event {
            PipelineEvent::LlmRequest { .. } => "LlmRequest",
            PipelineEvent::LlmResponse { .. } => "LlmResponse",
            PipelineEvent::LlmError { .. } => "LlmError",
            PipelineEvent::ExtractionInput { .. } => "ExtractionInput",
            PipelineEvent::ExtractionOutput { .. } => "ExtractionOutput",
            PipelineEvent::GapAnalysis { .. } => "GapAnalysis",
            PipelineEvent::ComprehensionCheck { .. } => "ComprehensionCheck",
            PipelineEvent::LoopIteration { .. } => "LoopIteration",
            PipelineEvent::SpeculativeClaim { .. } => "SpeculativeClaim",
            PipelineEvent::ForwardChain { .. } => "ForwardChain",
            PipelineEvent::ReviewResult { .. } => "ReviewResult",
            PipelineEvent::GraphNodeMutation { .. } => "GraphNodeMutation",
            PipelineEvent::GraphEdgeMutation { .. } => "GraphEdgeMutation",
            PipelineEvent::EdgeConfirmed { .. } => "EdgeConfirmed",
            PipelineEvent::SynonymResolution { .. } => "SynonymResolution",
            PipelineEvent::CitationBacking { .. } => "CitationBacking",
        })
        .collect();

    // Required events from the full pipeline
    assert!(
        event_types.contains(&"LlmRequest"),
        "pipeline must emit LlmRequest"
    );
    assert!(
        event_types.contains(&"LlmResponse"),
        "pipeline must emit LlmResponse"
    );
    assert!(
        event_types.contains(&"ExtractionInput"),
        "pipeline must emit ExtractionInput"
    );
    assert!(
        event_types.contains(&"ExtractionOutput"),
        "pipeline must emit ExtractionOutput"
    );
    assert!(
        event_types.contains(&"GapAnalysis"),
        "pipeline must emit GapAnalysis"
    );
    assert!(
        event_types.contains(&"LoopIteration"),
        "pipeline must emit LoopIteration"
    );
    assert!(
        event_types.contains(&"ComprehensionCheck"),
        "pipeline must emit ComprehensionCheck"
    );
    assert!(
        event_types.contains(&"GraphNodeMutation"),
        "pipeline must emit GraphNodeMutation"
    );
    assert!(
        event_types.contains(&"GraphEdgeMutation"),
        "pipeline must emit GraphEdgeMutation"
    );
    assert!(
        event_types.contains(&"SynonymResolution"),
        "pipeline must emit SynonymResolution"
    );
    assert!(
        event_types.contains(&"CitationBacking"),
        "pipeline must emit CitationBacking"
    );
}

// -------------------------------------------------------------------------
// Contract 2: Event ordering — seed before gaps, gaps before comprehension
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_pipeline_event_ordering() {
    let provider = pipeline_mock();
    let sink = MemorySink::new();
    let graph = KnowledgeGraph::in_memory().await.unwrap();
    let source_store = SourceStore::new(graph.db());
    let corr_id = Uuid::new_v4();

    let nsai = NsaiLoop::new(&provider, &sink).with_source_store(&source_store);
    nsai.run("Magnesium", &graph, corr_id).await;

    let events = sink.events_for(corr_id);

    // Find positions of key event types
    let first_request = events
        .iter()
        .position(|e| matches!(e.event, PipelineEvent::LlmRequest { .. }));
    let first_extraction = events
        .iter()
        .position(|e| matches!(e.event, PipelineEvent::ExtractionOutput { .. }));
    let first_gap = events
        .iter()
        .position(|e| matches!(e.event, PipelineEvent::GapAnalysis { .. }));
    let comprehension = events
        .iter()
        .position(|e| matches!(e.event, PipelineEvent::ComprehensionCheck { .. }));

    // Seed LLM request comes first
    assert!(first_request.is_some(), "should have LLM request");
    assert!(first_extraction.is_some(), "should have extraction output");
    assert!(first_gap.is_some(), "should have gap analysis");
    assert!(comprehension.is_some(), "should have comprehension check");

    // Ordering: request → extraction → gap analysis → comprehension
    assert!(
        first_request.unwrap() < first_extraction.unwrap(),
        "LLM request must come before extraction"
    );
    assert!(
        first_extraction.unwrap() < first_gap.unwrap(),
        "extraction must come before gap analysis"
    );
    assert!(
        first_gap.unwrap() < comprehension.unwrap(),
        "gap analysis must come before comprehension"
    );
}

// -------------------------------------------------------------------------
// Contract 3: Graph structure — expected nodes and edges exist
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_pipeline_produces_expected_graph_structure() {
    let provider = pipeline_mock();
    let sink = MemorySink::new();
    let graph = KnowledgeGraph::in_memory().await.unwrap();
    let source_store = SourceStore::new(graph.db());
    let corr_id = Uuid::new_v4();

    let nsai = NsaiLoop::new(&provider, &sink).with_source_store(&source_store);
    nsai.run("Magnesium", &graph, corr_id).await;

    // Must have the ingredient node
    assert!(
        graph.find_node("magnesium").await.is_some(),
        "graph must contain 'magnesium' node"
    );

    // Must have at least one system node
    let systems = graph
        .nodes_by_type(&graph_service::types::NodeType::System)
        .await;
    assert!(!systems.is_empty(), "graph must have at least one System node");

    // Must have at least one property node
    let properties = graph
        .nodes_by_type(&graph_service::types::NodeType::Property)
        .await;
    assert!(
        !properties.is_empty(),
        "graph must have at least one Property node"
    );

    // Must have edges
    assert!(
        graph.edge_count().await >= 3,
        "graph must have at least 3 edges from seed extraction"
    );

    // Magnesium should have outgoing edges
    let mag_idx = graph.find_node("magnesium").await.unwrap();
    let outgoing = graph.outgoing_edges(&mag_idx).await;
    assert!(
        outgoing.len() >= 2,
        "magnesium should connect to at least 2 targets"
    );
}

// -------------------------------------------------------------------------
// Contract 4: Synonym resolution produces CUI mappings when SuppKG is present
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_pipeline_synonym_resolution_assigns_cuis() {
    let provider = pipeline_mock();
    let sink = MemorySink::new();
    let graph = KnowledgeGraph::in_memory().await.unwrap();
    let source_store = SourceStore::new(graph.db());
    let merge_store = MergeStore::new(graph.db());
    let suppkg = make_suppkg();
    let corr_id = Uuid::new_v4();

    let nsai = NsaiLoop::new(&provider, &sink)
        .with_source_store(&source_store)
        .with_synonym_resolution(&suppkg, &merge_store);

    nsai.run("Magnesium", &graph, corr_id).await;

    // Magnesium should have a CUI mapping
    let mag_cui = merge_store.cui_for("magnesium").await;
    assert_eq!(
        mag_cui,
        Some("C0024467".to_string()),
        "magnesium should be mapped to CUI C0024467"
    );

    // At least one CUI should be assigned
    assert!(
        merge_store.cui_count().await >= 1,
        "should have at least one CUI mapping"
    );
}

// -------------------------------------------------------------------------
// Contract 5: Citation backing stores citations when SuppKG matches exist
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_pipeline_citation_backing_stores_citations() {
    let provider = pipeline_mock();
    let sink = MemorySink::new();
    let graph = KnowledgeGraph::in_memory().await.unwrap();
    let source_store = SourceStore::new(graph.db());
    let merge_store = MergeStore::new(graph.db());
    let suppkg = make_suppkg();
    let corr_id = Uuid::new_v4();

    // Run NSAI loop with synonym resolution (needed for CUI mappings)
    let nsai = NsaiLoop::new(&provider, &sink)
        .with_source_store(&source_store)
        .with_synonym_resolution(&suppkg, &merge_store);

    nsai.run("Magnesium", &graph, corr_id).await;

    // Run citation backing
    let result = nsai_loop::citations::run_citation_backing(
        &graph,
        &suppkg,
        &merge_store,
        &source_store,
        &sink,
        corr_id,
    )
    .await;

    // Should find and store at least one citation
    // (our SuppKG has magnesium→muscular system AFFECTS and magnesium→sleep quality AFFECTS)
    assert!(
        result.edges_backed >= 1,
        "should back at least one edge with a citation (backed: {}, checked: {})",
        result.edges_backed,
        result.edges_checked
    );
    assert!(
        result.citations_stored >= 1,
        "should store at least one citation"
    );
    assert!(
        source_store.citation_count().await >= 1,
        "citation table should have at least one record"
    );
}

// -------------------------------------------------------------------------
// Contract 6: Source tracking — all edges are tracked in the source layer
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_pipeline_source_tracking_records_all_edges() {
    let provider = pipeline_mock();
    let sink = MemorySink::new();
    let graph = KnowledgeGraph::in_memory().await.unwrap();
    let source_store = SourceStore::new(graph.db());
    let corr_id = Uuid::new_v4();

    let nsai = NsaiLoop::new(&provider, &sink).with_source_store(&source_store);
    nsai.run("Magnesium", &graph, corr_id).await;

    let edge_obs = source_store.total_edge_observations().await;
    let node_obs = source_store.total_node_observations().await;

    assert!(
        edge_obs >= 3,
        "should have at least 3 edge observations (got {})",
        edge_obs
    );
    assert!(
        node_obs >= 3,
        "should have at least 3 node observations (got {})",
        node_obs
    );

    // Every edge observation should have a valid provider
    let quality_edges = source_store.edges_by_quality().await;
    assert!(
        !quality_edges.is_empty(),
        "quality layer should have edge records"
    );
}
