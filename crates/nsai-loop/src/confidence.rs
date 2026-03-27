use graph_service::graph::KnowledgeGraph;
use graph_service::source::SourceStore;
use graph_service::types::{EdgeType, Source};

// ---------------------------------------------------------------------------
// Cross-provider confidence boosting
//
// Edges observed by multiple independent providers are stronger evidence.
// This function queries the source layer for multi-provider edges and
// applies a confidence boost to each one in the graph.
// ---------------------------------------------------------------------------

/// How much to boost confidence when multiple providers agree
const MULTI_PROVIDER_BOOST: f64 = 0.15;

/// Result of a confidence boosting pass
#[derive(Debug, Clone)]
pub struct BoostResult {
    /// Number of edges that qualified (observed by 2+ providers)
    pub multi_provider_edges: usize,
    /// Number of edges that were actually boosted (found in graph + updated)
    pub edges_boosted: usize,
}

/// Boost confidence on all edges observed by multiple providers.
///
/// Uses `SourceStore::multi_provider_edges()` to find edges confirmed by 2+
/// independent providers, then applies a flat boost to their confidence in
/// the graph (capped at 1.0).
pub async fn boost_multi_provider_confidence(
    graph: &KnowledgeGraph,
    source_store: &SourceStore,
) -> BoostResult {
    let multi = source_store.multi_provider_edges().await;
    let multi_provider_edges = multi.len();
    let mut edges_boosted = 0;

    for edge in &multi {
        let edge_type: EdgeType = match edge.edge_type.parse() {
            Ok(et) => et,
            Err(_) => continue,
        };

        // Look up the source and target nodes in the graph
        let src_idx = match graph.find_node(&edge.source_node).await {
            Some(idx) => idx,
            None => continue,
        };
        let tgt_idx = match graph.find_node(&edge.target_node).await {
            Some(idx) => idx,
            None => continue,
        };

        let updated = graph
            .boost_edge_confidence(&src_idx, &tgt_idx, &edge_type, MULTI_PROVIDER_BOOST)
            .await;

        if updated > 0 {
            edges_boosted += updated;
        }
    }

    BoostResult {
        multi_provider_edges,
        edges_boosted,
    }
}

// ---------------------------------------------------------------------------
// Confidence decay — unconfirmed speculative/deduced edges lose confidence
//
// Edges that were never independently confirmed should decay over time.
// Only edges with Source::StructurallyEmergent or Source::Deduced are
// eligible. Extracted edges don't decay — they came from an LLM response.
//
// Edges confirmed by multiple providers (via the source layer) are exempt
// because cross-provider agreement is strong evidence.
// ---------------------------------------------------------------------------

/// How much confidence to subtract per decay pass
const DECAY_AMOUNT: f64 = 0.05;

/// Minimum confidence — edges won't decay below this floor
const DECAY_FLOOR: f64 = 0.1;

/// Result of a confidence decay pass
#[derive(Debug, Clone)]
pub struct DecayResult {
    /// Number of edges eligible for decay (speculative/deduced, single provider)
    pub eligible: usize,
    /// Number of edges actually decayed (confidence was above floor)
    pub decayed: usize,
}

/// Decay confidence on unconfirmed speculative and deduced edges.
///
/// Eligible edges: Source::StructurallyEmergent or Source::Deduced,
/// AND not confirmed by multiple providers.
///
/// Each eligible edge loses `DECAY_AMOUNT` confidence per call, down to
/// `DECAY_FLOOR`. Call this once per NSAI loop iteration or once per
/// ingredient run — the caller decides the cadence.
pub async fn decay_unconfirmed_confidence(
    graph: &KnowledgeGraph,
    source_store: &SourceStore,
) -> DecayResult {
    // Get the set of multi-provider edges — these are exempt from decay
    let multi = source_store.multi_provider_edges().await;
    let multi_set: std::collections::HashSet<(String, String, String)> = multi
        .iter()
        .map(|e| {
            (
                e.source_node.clone(),
                e.target_node.clone(),
                e.edge_type.clone(),
            )
        })
        .collect();

    let all_edges = graph.all_edges().await;
    let mut eligible = 0;
    let mut decayed = 0;

    for (src, tgt, data) in &all_edges {
        // Only decay speculative and deduced edges
        match data.metadata.source {
            Source::StructurallyEmergent | Source::Deduced => {}
            Source::Extracted => continue,
        }

        // Look up node names for the multi-provider check
        let src_name = match graph.node_data(src).await {
            Some(d) => d.name,
            None => continue,
        };
        let tgt_name = match graph.node_data(tgt).await {
            Some(d) => d.name,
            None => continue,
        };
        let et_str = format!("{}", data.edge_type);

        // Exempt multi-provider edges
        if multi_set.contains(&(src_name, tgt_name, et_str)) {
            continue;
        }

        eligible += 1;

        // Only decay if above floor
        if data.metadata.confidence > DECAY_FLOOR {
            let new_conf = (data.metadata.confidence - DECAY_AMOUNT).max(DECAY_FLOOR);
            let updated = graph
                .set_edge_confidence(src, tgt, &data.edge_type, new_conf)
                .await;
            if updated > 0 {
                decayed += 1;
            }
        }
    }

    DecayResult { eligible, decayed }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use graph_service::types::*;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_boost_multi_provider_edges() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let store = SourceStore::new(graph.db());
        let now = Utc::now();

        // Create graph structure
        let mag = graph
            .add_node(NodeData::new("magnesium", NodeType::Ingredient))
            .await;
        let sys = graph
            .add_node(NodeData::new("muscular system", NodeType::System))
            .await;
        graph
            .add_edge(
                &mag,
                &sys,
                EdgeData::new(EdgeType::ActsOn, EdgeMetadata::extracted(0.7, 1, 0)),
            )
            .await;

        // Record observations from two providers
        store
            .record_edge_created(
                "magnesium",
                "muscular system",
                "acts_on",
                0.7,
                "Extracted",
                "anthropic",
                "claude-sonnet",
                Uuid::new_v4(),
                now,
            )
            .await;
        store
            .record_edge_confirmed(
                "magnesium",
                "muscular system",
                "acts_on",
                "gemini",
                "gemini-flash",
                Uuid::new_v4(),
                now,
            )
            .await;

        let result = boost_multi_provider_confidence(&graph, &store).await;

        assert_eq!(result.multi_provider_edges, 1);
        assert_eq!(result.edges_boosted, 1);

        // Verify confidence was boosted
        let edges = graph.outgoing_edges(&mag).await;
        let acts_on = edges
            .iter()
            .find(|(_, d)| d.edge_type == EdgeType::ActsOn)
            .unwrap();
        assert!(
            (acts_on.1.metadata.confidence - 0.85).abs() < 0.01,
            "confidence should be 0.7 + 0.15 = 0.85, got {}",
            acts_on.1.metadata.confidence
        );
    }

    #[tokio::test]
    async fn test_no_boost_single_provider() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let store = SourceStore::new(graph.db());
        let now = Utc::now();

        let mag = graph
            .add_node(NodeData::new("magnesium", NodeType::Ingredient))
            .await;
        let sys = graph
            .add_node(NodeData::new("nervous system", NodeType::System))
            .await;
        graph
            .add_edge(
                &mag,
                &sys,
                EdgeData::new(EdgeType::ActsOn, EdgeMetadata::extracted(0.7, 1, 0)),
            )
            .await;

        // Only one provider
        store
            .record_edge_created(
                "magnesium",
                "nervous system",
                "acts_on",
                0.7,
                "Extracted",
                "anthropic",
                "claude-sonnet",
                Uuid::new_v4(),
                now,
            )
            .await;

        let result = boost_multi_provider_confidence(&graph, &store).await;

        assert_eq!(result.multi_provider_edges, 0);
        assert_eq!(result.edges_boosted, 0);

        // Confidence unchanged
        let edges = graph.outgoing_edges(&mag).await;
        assert!((edges[0].1.metadata.confidence - 0.7).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_boost_caps_at_one() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let store = SourceStore::new(graph.db());
        let now = Utc::now();

        let mag = graph
            .add_node(NodeData::new("magnesium", NodeType::Ingredient))
            .await;
        let sys = graph
            .add_node(NodeData::new("muscular system", NodeType::System))
            .await;
        // Start at 0.95 — boost should cap at 1.0
        graph
            .add_edge(
                &mag,
                &sys,
                EdgeData::new(EdgeType::ActsOn, EdgeMetadata::extracted(0.95, 1, 0)),
            )
            .await;

        store
            .record_edge_created(
                "magnesium", "muscular system", "acts_on", 0.95, "Extracted",
                "anthropic", "claude-sonnet", Uuid::new_v4(), now,
            )
            .await;
        store
            .record_edge_confirmed(
                "magnesium", "muscular system", "acts_on",
                "gemini", "gemini-flash", Uuid::new_v4(), now,
            )
            .await;

        let result = boost_multi_provider_confidence(&graph, &store).await;
        assert_eq!(result.edges_boosted, 1);

        let edges = graph.outgoing_edges(&mag).await;
        assert!(
            edges[0].1.metadata.confidence <= 1.0,
            "confidence should not exceed 1.0"
        );
    }

    // -- Decay tests ----------------------------------------------------------

    #[tokio::test]
    async fn test_decay_speculative_edge() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let store = SourceStore::new(graph.db());

        let mag = graph
            .add_node(NodeData::new("magnesium", NodeType::Ingredient))
            .await;
        let prop = graph
            .add_node(NodeData::new("bone health", NodeType::Property))
            .await;
        // Speculative edge at 0.5
        graph
            .add_edge(
                &mag,
                &prop,
                EdgeData::new(EdgeType::Affords, EdgeMetadata::emergent(0.5, 1, 0)),
            )
            .await;

        let result = decay_unconfirmed_confidence(&graph, &store).await;

        assert_eq!(result.eligible, 1);
        assert_eq!(result.decayed, 1);

        let edges = graph.outgoing_edges(&mag).await;
        let affords = edges
            .iter()
            .find(|(_, d)| d.edge_type == EdgeType::Affords)
            .unwrap();
        assert!(
            (affords.1.metadata.confidence - 0.45).abs() < 0.01,
            "confidence should be 0.5 - 0.05 = 0.45, got {}",
            affords.1.metadata.confidence
        );
    }

    #[tokio::test]
    async fn test_decay_skips_extracted_edges() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let store = SourceStore::new(graph.db());

        let mag = graph
            .add_node(NodeData::new("magnesium", NodeType::Ingredient))
            .await;
        let sys = graph
            .add_node(NodeData::new("muscular system", NodeType::System))
            .await;
        // Extracted edge — should not decay
        graph
            .add_edge(
                &mag,
                &sys,
                EdgeData::new(EdgeType::ActsOn, EdgeMetadata::extracted(0.7, 1, 0)),
            )
            .await;

        let result = decay_unconfirmed_confidence(&graph, &store).await;

        assert_eq!(result.eligible, 0);
        assert_eq!(result.decayed, 0);

        let edges = graph.outgoing_edges(&mag).await;
        assert!(
            (edges[0].1.metadata.confidence - 0.7).abs() < 0.01,
            "extracted edge should not decay"
        );
    }

    #[tokio::test]
    async fn test_decay_respects_floor() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let store = SourceStore::new(graph.db());

        let mag = graph
            .add_node(NodeData::new("magnesium", NodeType::Ingredient))
            .await;
        let prop = graph
            .add_node(NodeData::new("bone health", NodeType::Property))
            .await;
        // Speculative edge already at floor
        graph
            .add_edge(
                &mag,
                &prop,
                EdgeData::new(EdgeType::Affords, EdgeMetadata::emergent(0.1, 1, 0)),
            )
            .await;

        let result = decay_unconfirmed_confidence(&graph, &store).await;

        assert_eq!(result.eligible, 1);
        assert_eq!(result.decayed, 0, "should not decay below floor");

        let edges = graph.outgoing_edges(&mag).await;
        assert!(
            (edges[0].1.metadata.confidence - 0.1).abs() < 0.01,
            "confidence should remain at floor"
        );
    }

    #[tokio::test]
    async fn test_decay_exempts_multi_provider() {
        let graph = KnowledgeGraph::in_memory().await.unwrap();
        let store = SourceStore::new(graph.db());
        let now = Utc::now();

        let mag = graph
            .add_node(NodeData::new("magnesium", NodeType::Ingredient))
            .await;
        let prop = graph
            .add_node(NodeData::new("bone health", NodeType::Property))
            .await;
        // Speculative edge, but confirmed by two providers
        graph
            .add_edge(
                &mag,
                &prop,
                EdgeData::new(EdgeType::Affords, EdgeMetadata::emergent(0.5, 1, 0)),
            )
            .await;

        store
            .record_edge_created(
                "magnesium", "bone health", "affords", 0.5, "StructurallyEmergent",
                "anthropic", "claude-sonnet", Uuid::new_v4(), now,
            )
            .await;
        store
            .record_edge_confirmed(
                "magnesium", "bone health", "affords",
                "gemini", "gemini-flash", Uuid::new_v4(), now,
            )
            .await;

        let result = decay_unconfirmed_confidence(&graph, &store).await;

        assert_eq!(result.eligible, 0, "multi-provider edge should be exempt");
        assert_eq!(result.decayed, 0);
    }
}
