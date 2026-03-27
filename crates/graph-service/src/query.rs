use std::collections::HashMap;

use crate::graph::{KnowledgeGraph, NodeIndex};
use crate::lens::ComplexityLens;
use crate::merge::MergeStore;
use crate::source::{EdgeQuality, EdgeWithQuality, SourceStore};
use crate::types::*;

// ---------------------------------------------------------------------------
// Query layer — pattern-based traversal with lens, quality, and scoring.
//
// Three-model consensus design (Claude, Gemini, Grok — 2026-03-24).
// See docs/QUERY_LAYER.md for rationale.
//
// Key decisions:
//   - Structured pattern matching for recommendations (not generic BFS)
//   - Geometric mean scoring (length-normalized)
//   - Eager quality map (one DB call at engine creation)
//   - Contraindications proactively attached to results
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Direction of traversal along an edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeDirection {
    /// Source → Target (following the edge's natural direction)
    Forward,
    /// Target ← Source (traversing against the edge's direction)
    Reverse,
}

/// A single step in a traversal path.
#[derive(Debug, Clone)]
pub enum PathStep {
    Node {
        index: NodeIndex,
        data: NodeData,
    },
    Edge {
        data: EdgeData,
        direction: EdgeDirection,
    },
}

/// A discovered path through the graph with a composite score.
#[derive(Debug, Clone)]
pub struct TraversalPath {
    /// Alternating Node, Edge, Node, Edge, ..., Node
    pub steps: Vec<PathStep>,
    /// Composite score (higher = more relevant/trustworthy)
    pub score: f64,
    /// Human-readable per-hop explanation fragments
    pub explanation: Vec<String>,
}

/// Controls traversal visibility and filtering.
#[derive(Debug, Clone)]
pub struct QueryConfig {
    pub lens: ComplexityLens,
    pub min_quality: Option<EdgeQuality>,
    pub max_depth: usize,
    pub min_confidence: Option<f64>,
    pub max_paths_per_result: usize,
}

impl Default for QueryConfig {
    fn default() -> Self {
        Self {
            lens: ComplexityLens::fifth_grade(),
            min_quality: None,
            max_depth: 4,
            min_confidence: None,
            max_paths_per_result: 3,
        }
    }
}

/// A recommendation result grouped by ingredient.
#[derive(Debug, Clone)]
pub struct RecommendationResult {
    pub ingredient: NodeData,
    pub ingredient_index: NodeIndex,
    pub paths: Vec<TraversalPath>,
    pub best_score: f64,
    pub weakest_quality: EdgeQuality,
    pub contraindications: Vec<TraversalPath>,
}

/// An effect result grouped by destination node.
#[derive(Debug, Clone)]
pub struct EffectResult {
    pub destination: NodeData,
    pub destination_index: NodeIndex,
    pub paths: Vec<TraversalPath>,
    pub best_score: f64,
    pub weakest_quality: EdgeQuality,
}

/// Named traversal patterns through the ontology.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecommendationPattern {
    /// Symptom →[presents_in]→ System ←[acts_on]← Ingredient
    DirectSystem,
    /// Symptom →[presents_in]→ System ←[modulates]← Mechanism ←[via_mechanism]← Ingredient
    ViaMechanism,
}

// ---------------------------------------------------------------------------
// Quality map — (source_node, target_node, edge_type) → EdgeQuality
// ---------------------------------------------------------------------------

type QualityKey = (String, String, String);

fn build_quality_map(edges: Vec<EdgeWithQuality>) -> HashMap<QualityKey, EdgeQuality> {
    edges
        .into_iter()
        .map(|e| {
            (
                (e.source_node, e.target_node, e.edge_type),
                e.quality,
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Path scoring
// ---------------------------------------------------------------------------

/// Quality tier → multiplier (weakest-link).
fn quality_bonus(q: EdgeQuality) -> f64 {
    match q {
        EdgeQuality::Deduced => 0.5,
        EdgeQuality::Speculative => 0.7,
        EdgeQuality::SingleProvider => 1.0,
        EdgeQuality::MultiProvider => 1.2,
        EdgeQuality::CitationBacked => 1.5,
    }
}

/// Length bias: rewards detail at high complexity, penalizes at low.
fn length_bias(lens_level: f64, path_edge_count: usize) -> f64 {
    1.0 + (lens_level - 0.5) * 0.25 * (path_edge_count as f64 - 2.0)
}

/// Specificity penalty for high-degree intermediate nodes. Nodes with many
/// connections are less informative as intermediate steps — a path through
/// "muscular system" (degree 8) is more specific than one through "body" (degree 40).
/// Only penalizes above a threshold; low-degree nodes get no penalty.
fn specificity_penalty(intermediate_degrees: &[usize]) -> f64 {
    const DEGREE_THRESHOLD: usize = 15;
    const PENALTY_MULTIPLIER: f64 = 0.6;

    let mut penalty = 1.0;
    for &degree in intermediate_degrees {
        if degree > DEGREE_THRESHOLD {
            penalty *= PENALTY_MULTIPLIER;
        }
    }
    penalty
}

/// Compute composite score for a path.
fn score_path(
    confidences: &[f64],
    weakest_quality: EdgeQuality,
    lens_level: f64,
) -> f64 {
    if confidences.is_empty() {
        return 0.0;
    }
    let n = confidences.len() as f64;
    let product: f64 = confidences.iter().product();
    let geo_mean = product.powf(1.0 / n);
    let qb = quality_bonus(weakest_quality);
    let lb = length_bias(lens_level, confidences.len());
    (geo_mean * qb * lb).max(0.0)
}

// ---------------------------------------------------------------------------
// Explanation generation
// ---------------------------------------------------------------------------

/// Generate a human-readable explanation for a traversal step.
/// `edge_source` and `edge_target` are always the edge's canonical source and
/// target (the real direction of the relationship), regardless of which way
/// the traversal walked. The explanation always reads: "A edge_type B".
fn explain_step(
    edge_source: &str,
    edge_type: &EdgeType,
    edge_target: &str,
    _direction: &EdgeDirection,
) -> String {
    format!("{} {} {}", edge_source, edge_type, edge_target)
}

// ---------------------------------------------------------------------------
// QueryEngine
// ---------------------------------------------------------------------------

pub struct QueryEngine<'a> {
    graph: &'a KnowledgeGraph,
    #[allow(dead_code)] // used in tests, will be used for citation lookups
    source: &'a SourceStore,
    merge: &'a MergeStore,
    quality_map: HashMap<QualityKey, EdgeQuality>,
}

impl<'a> QueryEngine<'a> {
    /// Build a query engine with an eager quality map (one DB call).
    pub async fn new(
        graph: &'a KnowledgeGraph,
        source: &'a SourceStore,
        merge: &'a MergeStore,
    ) -> Self {
        let quality_map = build_quality_map(source.edges_by_quality().await);
        Self {
            graph,
            source,
            merge,
            quality_map,
        }
    }

    /// Build a query engine with a pre-supplied quality map (for tests).
    #[cfg(test)]
    fn with_quality_map(
        graph: &'a KnowledgeGraph,
        source: &'a SourceStore,
        merge: &'a MergeStore,
        quality_map: HashMap<QualityKey, EdgeQuality>,
    ) -> Self {
        Self {
            graph,
            source,
            merge,
            quality_map,
        }
    }

    /// Look up quality for an edge. Returns None if no observations exist
    /// (falls back to Deduced).
    fn edge_quality(&self, node_a: &str, node_b: &str, edge_type: &str) -> EdgeQuality {
        let et = edge_type.to_string();
        let a = node_a.to_lowercase();
        let b = node_b.to_lowercase();
        // Try both orderings — the quality map is keyed by canonical (source→target)
        // but callers may pass nodes in traversal order which varies by query.
        self.quality_map
            .get(&(a.clone(), b.clone(), et.clone()))
            .or_else(|| self.quality_map.get(&(b, a, et)))
            .copied()
            .unwrap_or(EdgeQuality::Deduced)
    }

    /// Check if an edge passes the config's quality and confidence filters.
    fn edge_passes(
        &self,
        edge: &EdgeData,
        source_name: &str,
        target_name: &str,
        config: &QueryConfig,
    ) -> bool {
        // Lens check
        if !config.lens.can_see_edge(&edge.edge_type) {
            return false;
        }
        // Confidence check
        if let Some(min_conf) = config.min_confidence {
            if edge.metadata.confidence < min_conf {
                return false;
            }
        }
        // Quality check
        if let Some(min_q) = config.min_quality {
            let q = self.edge_quality(source_name, target_name, &edge.edge_type.to_string());
            if q < min_q {
                return false;
            }
        }
        true
    }

    /// Check if a node passes the lens filter.
    fn node_visible(&self, data: &NodeData, config: &QueryConfig) -> bool {
        config.lens.can_see_node(&data.node_type)
    }

    // -- Recommendation queries ------------------------------------------------

    /// "What ingredients address this symptom?"
    /// Runs all applicable RecommendationPatterns, groups by ingredient,
    /// attaches contraindications.
    pub async fn ingredients_for_symptom(
        &self,
        symptom: &str,
        config: &QueryConfig,
    ) -> Vec<RecommendationResult> {
        // Resolve through aliases
        let canonical = self.merge.resolve(symptom).await;
        let symptom_idx = match self.graph.find_node(&canonical).await {
            Some(idx) => idx,
            None => return vec![],
        };
        let symptom_data = match self.graph.node_data(&symptom_idx).await {
            Some(d) if d.node_type == NodeType::Symptom => d,
            _ => return vec![],
        };

        let mut all_paths: Vec<(NodeIndex, NodeData, TraversalPath)> = Vec::new();

        // Run each pattern
        let patterns = self.applicable_patterns(config);
        for pattern in patterns {
            match pattern {
                RecommendationPattern::DirectSystem => {
                    let paths = self
                        .pattern_direct_system(&symptom_idx, &symptom_data, config)
                        .await;
                    all_paths.extend(paths);
                }
                RecommendationPattern::ViaMechanism => {
                    let paths = self
                        .pattern_via_mechanism(&symptom_idx, &symptom_data, config)
                        .await;
                    all_paths.extend(paths);
                }
            }
        }

        self.group_and_rank(all_paths, config).await
    }

    /// "What ingredients act on this system?"
    pub async fn ingredients_for_system(
        &self,
        system: &str,
        config: &QueryConfig,
    ) -> Vec<RecommendationResult> {
        let canonical = self.merge.resolve(system).await;
        let system_idx = match self.graph.find_node(&canonical).await {
            Some(idx) => idx,
            None => return vec![],
        };
        let system_data = match self.graph.node_data(&system_idx).await {
            Some(d) if d.node_type == NodeType::System => d,
            _ => return vec![],
        };

        let mut all_paths: Vec<(NodeIndex, NodeData, TraversalPath)> = Vec::new();

        // Direct: System ←[acts_on]← Ingredient
        let incoming = self.graph.incoming_edges(&system_idx).await;
        for (src_idx, edge) in &incoming {
            if edge.edge_type != EdgeType::ActsOn {
                continue;
            }
            let src_data = match self.graph.node_data(src_idx).await {
                Some(d) => d,
                None => continue,
            };
            if src_data.node_type != NodeType::Ingredient {
                continue;
            }
            if !self.edge_passes(edge, &src_data.name, &system_data.name, config) {
                continue;
            }

            let explanation = vec![explain_step(
                &src_data.name,
                &edge.edge_type,
                &system_data.name,
                &EdgeDirection::Reverse,
            )];
            let confidences = [edge.metadata.confidence];
            let weakest = self.edge_quality(
                &src_data.name,
                &system_data.name,
                &edge.edge_type.to_string(),
            );
            let score = score_path(&confidences, weakest, config.lens.level());

            let path = TraversalPath {
                steps: vec![
                    PathStep::Node {
                        index: src_idx.clone(),
                        data: src_data.clone(),
                    },
                    PathStep::Edge {
                        data: edge.clone(),
                        direction: EdgeDirection::Reverse,
                    },
                    PathStep::Node {
                        index: system_idx.clone(),
                        data: system_data.clone(),
                    },
                ],
                score,
                explanation,
            };
            all_paths.push((src_idx.clone(), src_data, path));
        }

        self.group_and_rank(all_paths, config).await
    }

    /// "What does this ingredient do?" (forward BFS, grouped by destination node)
    pub async fn effects_of_ingredient(
        &self,
        ingredient: &str,
        config: &QueryConfig,
    ) -> Vec<EffectResult> {
        let canonical = self.merge.resolve(ingredient).await;
        let start_idx = match self.graph.find_node(&canonical).await {
            Some(idx) => idx,
            None => return vec![],
        };
        let start_data = match self.graph.node_data(&start_idx).await {
            Some(d) if d.node_type == NodeType::Ingredient => d,
            _ => return vec![],
        };

        let initial = vec![(
            start_idx.clone(),
            start_data.clone(),
            vec![PathStep::Node {
                index: start_idx.clone(),
                data: start_data.clone(),
            }],
            vec![],     // confidences
            vec![],     // explanations
        )];

        // Collect (destination_idx, destination_data, path) tuples
        let mut all_paths: Vec<(NodeIndex, NodeData, TraversalPath)> = Vec::new();
        let mut frontier = initial;

        for _depth in 0..config.max_depth {
            let mut next_frontier = Vec::new();

            for (current_idx, current_data, steps, confidences, explanations) in &frontier {
                let outgoing = self.graph.outgoing_edges(current_idx).await;

                for (tgt_idx, edge) in &outgoing {
                    let tgt_data = match self.graph.node_data(tgt_idx).await {
                        Some(d) => d,
                        None => continue,
                    };

                    if !self.node_visible(&tgt_data, config) {
                        continue;
                    }
                    if !self.edge_passes(edge, &current_data.name, &tgt_data.name, config) {
                        continue;
                    }

                    // Avoid cycles
                    let already_visited = steps.iter().any(|s| matches!(s, PathStep::Node { index, .. } if *index == *tgt_idx));
                    if already_visited {
                        continue;
                    }

                    let mut new_steps = steps.clone();
                    new_steps.push(PathStep::Edge {
                        data: edge.clone(),
                        direction: EdgeDirection::Forward,
                    });
                    new_steps.push(PathStep::Node {
                        index: tgt_idx.clone(),
                        data: tgt_data.clone(),
                    });

                    let mut new_conf = confidences.clone();
                    new_conf.push(edge.metadata.confidence);

                    let mut new_expl = explanations.clone();
                    new_expl.push(explain_step(
                        &current_data.name,
                        &edge.edge_type,
                        &tgt_data.name,
                        &EdgeDirection::Forward,
                    ));

                    let weakest = new_conf
                        .iter()
                        .enumerate()
                        .map(|(i, _)| {
                            self.quality_from_steps(&new_steps, i)
                        })
                        .min()
                        .unwrap_or(EdgeQuality::Deduced);

                    // Compute specificity penalty for intermediate nodes
                    // (all nodes except first and last in the path)
                    let mut intermediate_degrees = Vec::new();
                    let end = new_steps.len().saturating_sub(2);
                    if end > 2 {
                        for step in &new_steps[2..end] {
                            if let PathStep::Node { index: idx, .. } = step {
                                intermediate_degrees.push(self.graph.node_degree(idx).await);
                            }
                        }
                    }
                    let sp = specificity_penalty(&intermediate_degrees);
                    let score = score_path(&new_conf, weakest, config.lens.level()) * sp;

                    let path = TraversalPath {
                        steps: new_steps.clone(),
                        score,
                        explanation: new_expl.clone(),
                    };

                    all_paths.push((tgt_idx.clone(), tgt_data.clone(), path));

                    next_frontier.push((
                        tgt_idx.clone(),
                        tgt_data,
                        new_steps,
                        new_conf,
                        new_expl,
                    ));
                }
            }

            if next_frontier.is_empty() {
                break;
            }
            frontier = next_frontier;
        }

        // Group by destination node
        let mut groups: HashMap<NodeIndex, (NodeData, Vec<TraversalPath>)> = HashMap::new();
        for (idx, data, path) in all_paths {
            groups
                .entry(idx)
                .or_insert_with(|| (data, Vec::new()))
                .1
                .push(path);
        }

        let mut results: Vec<EffectResult> = Vec::new();
        for (idx, (data, mut paths)) in groups {
            paths.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            paths.truncate(config.max_paths_per_result);

            let best_score = paths.first().map(|p| p.score).unwrap_or(0.0);
            let weakest_quality = self.weakest_quality_in_path(paths.first().unwrap());

            results.push(EffectResult {
                destination: data,
                destination_index: idx,
                paths,
                best_score,
                weakest_quality,
            });
        }

        results.sort_by(|a, b| {
            b.best_score
                .partial_cmp(&a.best_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results
    }

    // -- Pattern implementations -----------------------------------------------

    /// Which patterns are applicable at this lens level?
    fn applicable_patterns(&self, config: &QueryConfig) -> Vec<RecommendationPattern> {
        let mut patterns = vec![];
        // DirectSystem needs: presents_in + acts_on (both foundational, always visible)
        if config.lens.can_see_edge(&EdgeType::PresentsIn)
            && config.lens.can_see_edge(&EdgeType::ActsOn)
        {
            patterns.push(RecommendationPattern::DirectSystem);
        }
        // ViaMechanism needs: presents_in + modulates + via_mechanism
        if config.lens.can_see_edge(&EdgeType::PresentsIn)
            && config.lens.can_see_edge(&EdgeType::Modulates)
            && config.lens.can_see_edge(&EdgeType::ViaMechanism)
        {
            patterns.push(RecommendationPattern::ViaMechanism);
        }
        patterns
    }

    /// Pattern: Symptom →[presents_in]→ System ←[acts_on]← Ingredient
    async fn pattern_direct_system(
        &self,
        symptom_idx: &NodeIndex,
        symptom_data: &NodeData,
        config: &QueryConfig,
    ) -> Vec<(NodeIndex, NodeData, TraversalPath)> {
        let mut results = Vec::new();

        // Hop 1: Symptom → System via presents_in
        let outgoing = self.graph.outgoing_edges(symptom_idx).await;
        for (system_idx, edge1) in &outgoing {
            if edge1.edge_type != EdgeType::PresentsIn {
                continue;
            }
            let system_data = match self.graph.node_data(system_idx).await {
                Some(d) if d.node_type == NodeType::System => d,
                _ => continue,
            };
            if !self.edge_passes(edge1, &symptom_data.name, &system_data.name, config) {
                continue;
            }

            // Hop 2: System ← Ingredient via acts_on (reverse)
            let incoming = self.graph.incoming_edges(system_idx).await;
            for (ingr_idx, edge2) in &incoming {
                if edge2.edge_type != EdgeType::ActsOn {
                    continue;
                }
                let ingr_data = match self.graph.node_data(ingr_idx).await {
                    Some(d) if d.node_type == NodeType::Ingredient => d,
                    _ => continue,
                };
                if !self.edge_passes(edge2, &ingr_data.name, &system_data.name, config) {
                    continue;
                }

                let confidences = [edge1.metadata.confidence, edge2.metadata.confidence];
                let q1 = self.edge_quality(
                    &symptom_data.name,
                    &system_data.name,
                    &edge1.edge_type.to_string(),
                );
                let q2 = self.edge_quality(
                    &ingr_data.name,
                    &system_data.name,
                    &edge2.edge_type.to_string(),
                );
                let weakest = q1.min(q2);
                // System node is intermediate — penalize supernodes
                let system_degree = self.graph.node_degree(system_idx).await;
                let sp = specificity_penalty(&[system_degree]);
                let score = score_path(&confidences, weakest, config.lens.level()) * sp;

                let explanation = vec![
                    explain_step(
                        &symptom_data.name,
                        &edge1.edge_type,
                        &system_data.name,
                        &EdgeDirection::Forward,
                    ),
                    explain_step(
                        &ingr_data.name,
                        &edge2.edge_type,
                        &system_data.name,
                        &EdgeDirection::Reverse,
                    ),
                ];

                let path = TraversalPath {
                    steps: vec![
                        PathStep::Node {
                            index: symptom_idx.clone(),
                            data: symptom_data.clone(),
                        },
                        PathStep::Edge {
                            data: edge1.clone(),
                            direction: EdgeDirection::Forward,
                        },
                        PathStep::Node {
                            index: system_idx.clone(),
                            data: system_data.clone(),
                        },
                        PathStep::Edge {
                            data: edge2.clone(),
                            direction: EdgeDirection::Reverse,
                        },
                        PathStep::Node {
                            index: ingr_idx.clone(),
                            data: ingr_data.clone(),
                        },
                    ],
                    score,
                    explanation,
                };

                results.push((ingr_idx.clone(), ingr_data, path));
            }
        }

        results
    }

    /// Pattern: Symptom →[presents_in]→ System ←[modulates]← Mechanism ←[via_mechanism]← Ingredient
    async fn pattern_via_mechanism(
        &self,
        symptom_idx: &NodeIndex,
        symptom_data: &NodeData,
        config: &QueryConfig,
    ) -> Vec<(NodeIndex, NodeData, TraversalPath)> {
        let mut results = Vec::new();

        // Hop 1: Symptom → System via presents_in
        let outgoing = self.graph.outgoing_edges(symptom_idx).await;
        for (system_idx, edge1) in &outgoing {
            if edge1.edge_type != EdgeType::PresentsIn {
                continue;
            }
            let system_data = match self.graph.node_data(system_idx).await {
                Some(d) if d.node_type == NodeType::System => d,
                _ => continue,
            };
            if !self.edge_passes(edge1, &symptom_data.name, &system_data.name, config) {
                continue;
            }

            // Hop 2: System ← Mechanism via modulates (reverse)
            let sys_incoming = self.graph.incoming_edges(system_idx).await;
            for (mech_idx, edge2) in &sys_incoming {
                if edge2.edge_type != EdgeType::Modulates {
                    continue;
                }
                let mech_data = match self.graph.node_data(mech_idx).await {
                    Some(d) if d.node_type == NodeType::Mechanism => d,
                    _ => continue,
                };
                if !self.node_visible(&mech_data, config) {
                    continue;
                }
                if !self.edge_passes(edge2, &mech_data.name, &system_data.name, config) {
                    continue;
                }

                // Hop 3: Mechanism ← Ingredient via via_mechanism (reverse)
                let mech_incoming = self.graph.incoming_edges(mech_idx).await;
                for (ingr_idx, edge3) in &mech_incoming {
                    if edge3.edge_type != EdgeType::ViaMechanism {
                        continue;
                    }
                    let ingr_data = match self.graph.node_data(ingr_idx).await {
                        Some(d) if d.node_type == NodeType::Ingredient => d,
                        _ => continue,
                    };
                    if !self.edge_passes(edge3, &ingr_data.name, &mech_data.name, config) {
                        continue;
                    }

                    let confidences = [
                        edge1.metadata.confidence,
                        edge2.metadata.confidence,
                        edge3.metadata.confidence,
                    ];
                    let q1 = self.edge_quality(
                        &symptom_data.name,
                        &system_data.name,
                        &edge1.edge_type.to_string(),
                    );
                    let q2 = self.edge_quality(
                        &mech_data.name,
                        &system_data.name,
                        &edge2.edge_type.to_string(),
                    );
                    let q3 = self.edge_quality(
                        &ingr_data.name,
                        &mech_data.name,
                        &edge3.edge_type.to_string(),
                    );
                    let weakest = q1.min(q2).min(q3);
                    // System and Mechanism are both intermediate — penalize supernodes
                    let system_degree = self.graph.node_degree(system_idx).await;
                    let mech_degree = self.graph.node_degree(mech_idx).await;
                    let sp = specificity_penalty(&[system_degree, mech_degree]);
                    let score = score_path(&confidences, weakest, config.lens.level()) * sp;

                    let explanation = vec![
                        explain_step(
                            &symptom_data.name,
                            &edge1.edge_type,
                            &system_data.name,
                            &EdgeDirection::Forward,
                        ),
                        explain_step(
                            &mech_data.name,
                            &edge2.edge_type,
                            &system_data.name,
                            &EdgeDirection::Reverse,
                        ),
                        explain_step(
                            &ingr_data.name,
                            &edge3.edge_type,
                            &mech_data.name,
                            &EdgeDirection::Reverse,
                        ),
                    ];

                    let path = TraversalPath {
                        steps: vec![
                            PathStep::Node {
                                index: symptom_idx.clone(),
                                data: symptom_data.clone(),
                            },
                            PathStep::Edge {
                                data: edge1.clone(),
                                direction: EdgeDirection::Forward,
                            },
                            PathStep::Node {
                                index: system_idx.clone(),
                                data: system_data.clone(),
                            },
                            PathStep::Edge {
                                data: edge2.clone(),
                                direction: EdgeDirection::Reverse,
                            },
                            PathStep::Node {
                                index: mech_idx.clone(),
                                data: mech_data.clone(),
                            },
                            PathStep::Edge {
                                data: edge3.clone(),
                                direction: EdgeDirection::Reverse,
                            },
                            PathStep::Node {
                                index: ingr_idx.clone(),
                                data: ingr_data.clone(),
                            },
                        ],
                        score,
                        explanation,
                    };

                    results.push((ingr_idx.clone(), ingr_data, path));
                }
            }
        }

        results
    }

    // -- Grouping, ranking, and contraindication attachment ---------------------

    /// Group paths by ingredient, rank, attach contraindications.
    async fn group_and_rank(
        &self,
        paths: Vec<(NodeIndex, NodeData, TraversalPath)>,
        config: &QueryConfig,
    ) -> Vec<RecommendationResult> {
        // Group by ingredient node index
        let mut groups: HashMap<NodeIndex, (NodeData, Vec<TraversalPath>)> = HashMap::new();
        for (idx, data, path) in paths {
            groups
                .entry(idx)
                .or_insert_with(|| (data, Vec::new()))
                .1
                .push(path);
        }

        let mut results: Vec<RecommendationResult> = Vec::new();

        for (idx, (data, mut paths)) in groups {
            // Sort paths by score descending
            paths.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            // Keep top-N
            paths.truncate(config.max_paths_per_result);

            let best_score = paths.first().map(|p| p.score).unwrap_or(0.0);

            // Weakest quality across the best path's edges
            let weakest_quality = self.weakest_quality_in_path(paths.first().unwrap());

            // Proactively check contraindications
            let contraindications = self.find_contraindications(&idx, config).await;

            results.push(RecommendationResult {
                ingredient: data,
                ingredient_index: idx,
                paths,
                best_score,
                weakest_quality,
                contraindications,
            });
        }

        // Sort results by best_score descending
        results.sort_by(|a, b| {
            b.best_score
                .partial_cmp(&a.best_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        results
    }

    /// Find contraindicated_with edges for an ingredient.
    async fn find_contraindications(
        &self,
        ingredient_idx: &NodeIndex,
        config: &QueryConfig,
    ) -> Vec<TraversalPath> {
        // Only check if contraindicated_with is visible at this lens level
        if !config.lens.can_see_edge(&EdgeType::ContraindicatedWith) {
            return vec![];
        }

        let ingr_data = match self.graph.node_data(ingredient_idx).await {
            Some(d) => d,
            None => return vec![],
        };

        let mut contra_paths = Vec::new();

        // Check outgoing contraindicated_with edges
        let outgoing = self.graph.outgoing_edges(ingredient_idx).await;
        for (tgt_idx, edge) in &outgoing {
            if edge.edge_type != EdgeType::ContraindicatedWith {
                continue;
            }
            let tgt_data = match self.graph.node_data(tgt_idx).await {
                Some(d) => d,
                None => continue,
            };

            let explanation = vec![explain_step(
                &ingr_data.name,
                &edge.edge_type,
                &tgt_data.name,
                &EdgeDirection::Forward,
            )];

            contra_paths.push(TraversalPath {
                steps: vec![
                    PathStep::Node {
                        index: ingredient_idx.clone(),
                        data: ingr_data.clone(),
                    },
                    PathStep::Edge {
                        data: edge.clone(),
                        direction: EdgeDirection::Forward,
                    },
                    PathStep::Node {
                        index: tgt_idx.clone(),
                        data: tgt_data,
                    },
                ],
                score: edge.metadata.confidence,
                explanation,
            });
        }

        // Also check incoming (contraindication can go either direction)
        let incoming = self.graph.incoming_edges(ingredient_idx).await;
        for (src_idx, edge) in &incoming {
            if edge.edge_type != EdgeType::ContraindicatedWith {
                continue;
            }
            let src_data = match self.graph.node_data(src_idx).await {
                Some(d) => d,
                None => continue,
            };

            let explanation = vec![explain_step(
                &src_data.name,
                &edge.edge_type,
                &ingr_data.name,
                &EdgeDirection::Reverse,
            )];

            contra_paths.push(TraversalPath {
                steps: vec![
                    PathStep::Node {
                        index: src_idx.clone(),
                        data: src_data,
                    },
                    PathStep::Edge {
                        data: edge.clone(),
                        direction: EdgeDirection::Reverse,
                    },
                    PathStep::Node {
                        index: ingredient_idx.clone(),
                        data: ingr_data.clone(),
                    },
                ],
                score: edge.metadata.confidence,
                explanation,
            });
        }

        contra_paths
    }

    /// Compute weakest quality tier across all edges in a path.
    fn weakest_quality_in_path(&self, path: &TraversalPath) -> EdgeQuality {
        let mut weakest = EdgeQuality::CitationBacked;
        let mut prev_node_name: Option<String> = None;

        for step in &path.steps {
            match step {
                PathStep::Node { data, .. } => {
                    prev_node_name = Some(data.name.clone());
                }
                PathStep::Edge { data: _, direction } => {
                    // We need source and target names. The previous node is one end;
                    // the next node is the other. Since we process linearly, we
                    // peek at the stored edge type and use the direction to figure
                    // out the quality key.
                    if let Some(ref prev_name) = prev_node_name {
                        // For quality lookup we need the canonical source→target
                        // (the direction the edge was originally stored)
                        let (src, tgt) = match direction {
                            EdgeDirection::Forward => (prev_name.clone(), String::new()),
                            EdgeDirection::Reverse => (String::new(), prev_name.clone()),
                        };
                        // We'll use a simpler approach: iterate steps in pairs
                        let _ = src;
                        let _ = tgt;
                    }
                }
            }
        }

        // Simpler: iterate steps looking at Node-Edge-Node triples
        let steps = &path.steps;
        let mut i = 0;
        while i + 2 < steps.len() {
            if let (
                PathStep::Node { data: n1, .. },
                PathStep::Edge {
                    data: edge,
                    direction,
                },
                PathStep::Node { data: n2, .. },
            ) = (&steps[i], &steps[i + 1], &steps[i + 2])
            {
                let (src, tgt) = match direction {
                    EdgeDirection::Forward => (&n1.name, &n2.name),
                    EdgeDirection::Reverse => (&n2.name, &n1.name),
                };
                let q = self.edge_quality(src, tgt, &edge.edge_type.to_string());
                weakest = weakest.min(q);
            }
            i += 2; // advance by one edge (Node-Edge pair)
        }

        weakest
    }

    /// Helper for effects_of_ingredient: get quality for the i-th edge in a path.
    fn quality_from_steps(&self, steps: &[PathStep], edge_index: usize) -> EdgeQuality {
        // Edge at position edge_index is at steps[edge_index * 2 + 1]
        // Surrounding nodes at steps[edge_index * 2] and steps[edge_index * 2 + 2]
        let step_pos = edge_index * 2 + 1;
        if step_pos + 1 >= steps.len() {
            return EdgeQuality::Deduced;
        }

        if let (
            PathStep::Node { data: n1, .. },
            PathStep::Edge {
                data: edge,
                direction,
            },
            PathStep::Node { data: n2, .. },
        ) = (&steps[step_pos - 1], &steps[step_pos], &steps[step_pos + 1])
        {
            let (src, tgt) = match direction {
                EdgeDirection::Forward => (&n1.name, &n2.name),
                EdgeDirection::Reverse => (&n2.name, &n1.name),
            };
            self.edge_quality(src, tgt, &edge.edge_type.to_string())
        } else {
            EdgeQuality::Deduced
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a test graph:
    ///   Muscle Cramps (Symptom) →[presents_in]→ Muscular System (System)
    ///   Magnesium (Ingredient) →[acts_on]→ Muscular System (System)
    ///   Magnesium (Ingredient) →[via_mechanism]→ Smooth Muscle Relaxation (Mechanism)
    ///   Smooth Muscle Relaxation (Mechanism) →[modulates]→ Muscular System (System)
    ///   Magnesium (Ingredient) →[affords]→ Muscle Relaxation (Property)
    ///   Zinc (Ingredient) →[acts_on]→ Muscular System (System)
    async fn build_test_graph() -> (KnowledgeGraph, SourceStore, MergeStore) {
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        let source = SourceStore::new(kg.db());
        let merge = MergeStore::new(kg.db());

        let cramps = kg
            .add_node(NodeData::new("Muscle Cramps", NodeType::Symptom))
            .await;
        let muscular = kg
            .add_node(NodeData::new("Muscular System", NodeType::System))
            .await;
        let mag = kg
            .add_node(NodeData::new("Magnesium", NodeType::Ingredient))
            .await;
        let relaxation = kg
            .add_node(NodeData::new("Smooth Muscle Relaxation", NodeType::Mechanism))
            .await;
        let prop = kg
            .add_node(NodeData::new("Muscle Relaxation", NodeType::Property))
            .await;
        let zinc = kg
            .add_node(NodeData::new("Zinc", NodeType::Ingredient))
            .await;

        // Symptom → System
        kg.add_edge(
            &cramps,
            &muscular,
            EdgeData::new(EdgeType::PresentsIn, EdgeMetadata::extracted(0.95, 1, 0)),
        )
        .await;

        // Ingredient → System
        kg.add_edge(
            &mag,
            &muscular,
            EdgeData::new(EdgeType::ActsOn, EdgeMetadata::extracted(0.90, 1, 0)),
        )
        .await;

        // Ingredient → Mechanism
        kg.add_edge(
            &mag,
            &relaxation,
            EdgeData::new(EdgeType::ViaMechanism, EdgeMetadata::extracted(0.85, 1, 0)),
        )
        .await;

        // Mechanism → System
        kg.add_edge(
            &relaxation,
            &muscular,
            EdgeData::new(EdgeType::Modulates, EdgeMetadata::extracted(0.88, 1, 0)),
        )
        .await;

        // Ingredient → Property
        kg.add_edge(
            &mag,
            &prop,
            EdgeData::new(EdgeType::Affords, EdgeMetadata::extracted(0.92, 1, 0)),
        )
        .await;

        // Second ingredient → same system (lower confidence)
        kg.add_edge(
            &zinc,
            &muscular,
            EdgeData::new(EdgeType::ActsOn, EdgeMetadata::extracted(0.60, 1, 0)),
        )
        .await;

        (kg, source, merge)
    }

    #[tokio::test]
    async fn test_ingredients_for_symptom_direct_pattern() {
        let (kg, source, merge) = build_test_graph().await;
        let engine = QueryEngine::new(&kg, &source, &merge).await;

        let config = QueryConfig {
            lens: ComplexityLens::fifth_grade(),
            ..Default::default()
        };

        let results = engine
            .ingredients_for_symptom("Muscle Cramps", &config)
            .await;

        assert!(!results.is_empty(), "should find at least one ingredient");

        let names: Vec<&str> = results.iter().map(|r| r.ingredient.name.as_str()).collect();
        assert!(names.contains(&"Magnesium"), "should find Magnesium");
        assert!(names.contains(&"Zinc"), "should find Zinc");

        // Magnesium should rank higher (0.90 confidence vs 0.60)
        assert_eq!(results[0].ingredient.name, "Magnesium");
    }

    #[tokio::test]
    async fn test_ingredients_for_symptom_via_mechanism_pattern() {
        let (kg, source, merge) = build_test_graph().await;
        let engine = QueryEngine::new(&kg, &source, &merge).await;

        // At fifth grade, both DirectSystem and ViaMechanism patterns are visible
        // (all edge types involved are foundational at 0.0)
        let config = QueryConfig {
            lens: ComplexityLens::fifth_grade(),
            ..Default::default()
        };

        let results = engine
            .ingredients_for_symptom("Muscle Cramps", &config)
            .await;

        // Magnesium should appear via both patterns
        let mag_result = results
            .iter()
            .find(|r| r.ingredient.name == "Magnesium")
            .expect("should find Magnesium");

        // Should have paths from both direct (2 edges) and via_mechanism (3 edges)
        assert!(
            mag_result.paths.len() >= 2,
            "Magnesium should have paths from multiple patterns, got {}",
            mag_result.paths.len()
        );
    }

    #[tokio::test]
    async fn test_effects_of_ingredient() {
        let (kg, source, merge) = build_test_graph().await;
        let engine = QueryEngine::new(&kg, &source, &merge).await;

        let config = QueryConfig {
            lens: ComplexityLens::fifth_grade(),
            ..Default::default()
        };

        let results = engine.effects_of_ingredient("Magnesium", &config).await;

        assert!(!results.is_empty(), "should find effects");

        // Should reach Muscular System, Smooth Muscle Relaxation, Muscle Relaxation
        let destinations: Vec<&str> = results
            .iter()
            .map(|r| r.destination.name.as_str())
            .collect();

        assert!(
            destinations.contains(&"Muscular System"),
            "should reach Muscular System"
        );
        assert!(
            destinations.contains(&"Muscle Relaxation"),
            "should reach Muscle Relaxation property"
        );

        // Muscular System should be reachable via multiple paths
        // (direct acts_on + via Smooth Muscle Relaxation → modulates)
        let muscular = results
            .iter()
            .find(|r| r.destination.name == "Muscular System")
            .unwrap();
        assert!(
            muscular.paths.len() >= 2,
            "Muscular System should be reachable via multiple paths, got {}",
            muscular.paths.len()
        );
    }

    #[tokio::test]
    async fn test_ingredients_for_system() {
        let (kg, source, merge) = build_test_graph().await;
        let engine = QueryEngine::new(&kg, &source, &merge).await;

        let config = QueryConfig::default();
        let results = engine
            .ingredients_for_system("Muscular System", &config)
            .await;

        assert_eq!(results.len(), 2, "two ingredients act on muscular system");
        // Magnesium first (0.90 > 0.60)
        assert_eq!(results[0].ingredient.name, "Magnesium");
        assert_eq!(results[1].ingredient.name, "Zinc");
    }

    #[tokio::test]
    async fn test_min_confidence_filter() {
        let (kg, source, merge) = build_test_graph().await;
        let engine = QueryEngine::new(&kg, &source, &merge).await;

        let config = QueryConfig {
            lens: ComplexityLens::fifth_grade(),
            min_confidence: Some(0.80),
            ..Default::default()
        };

        let results = engine
            .ingredients_for_symptom("Muscle Cramps", &config)
            .await;

        // Zinc (0.60 confidence) should be filtered out
        let names: Vec<&str> = results.iter().map(|r| r.ingredient.name.as_str()).collect();
        assert!(names.contains(&"Magnesium"));
        assert!(!names.contains(&"Zinc"), "Zinc should be filtered by min_confidence");
    }

    #[tokio::test]
    async fn test_unknown_symptom_returns_empty() {
        let (kg, source, merge) = build_test_graph().await;
        let engine = QueryEngine::new(&kg, &source, &merge).await;

        let config = QueryConfig::default();
        let results = engine
            .ingredients_for_symptom("Nonexistent Symptom", &config)
            .await;

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_alias_resolution() {
        let (kg, source, merge) = build_test_graph().await;

        // Register alias: "leg cramps" → "muscle cramps"
        merge
            .record_alias("muscle cramps", "leg cramps", 0.95, "test")
            .await;

        let engine = QueryEngine::new(&kg, &source, &merge).await;
        let config = QueryConfig::default();

        let results = engine
            .ingredients_for_symptom("leg cramps", &config)
            .await;

        assert!(!results.is_empty(), "alias should resolve to muscle cramps");
    }

    #[tokio::test]
    async fn test_explanation_generated() {
        let (kg, source, merge) = build_test_graph().await;
        let engine = QueryEngine::new(&kg, &source, &merge).await;

        let config = QueryConfig::default();
        let results = engine
            .ingredients_for_symptom("Muscle Cramps", &config)
            .await;

        let mag = results
            .iter()
            .find(|r| r.ingredient.name == "Magnesium")
            .unwrap();

        let first_path = &mag.paths[0];
        assert!(
            !first_path.explanation.is_empty(),
            "explanation should not be empty"
        );
        // Should mention the symptom and the system
        let joined = first_path.explanation.join(" | ");
        assert!(
            joined.contains("Muscle Cramps") && joined.contains("Muscular System"),
            "explanation should reference symptom and system: {}",
            joined
        );
    }

    #[tokio::test]
    async fn test_contraindications_attached() {
        let (kg, source, merge) = build_test_graph().await;

        // Add a contraindication: Magnesium contraindicated_with SomeCondition
        let mag_idx = kg.find_node("Magnesium").await.unwrap();
        let condition = kg
            .add_node(NodeData::new("Hemophilia", NodeType::Condition))
            .await;
        kg.add_edge(
            &mag_idx,
            &condition,
            EdgeData::new(
                EdgeType::ContraindicatedWith,
                EdgeMetadata::extracted(0.80, 1, 0),
            ),
        )
        .await;

        let engine = QueryEngine::new(&kg, &source, &merge).await;

        // contraindicated_with has min_complexity 0.3, so use tenth_grade lens
        let config = QueryConfig {
            lens: ComplexityLens::tenth_grade(),
            ..Default::default()
        };

        let results = engine
            .ingredients_for_symptom("Muscle Cramps", &config)
            .await;

        let mag = results
            .iter()
            .find(|r| r.ingredient.name == "Magnesium")
            .unwrap();

        assert!(
            !mag.contraindications.is_empty(),
            "should have contraindications attached"
        );
    }

    #[tokio::test]
    async fn test_scoring_geometric_mean() {
        // Two edges at 0.81 each: geo mean = 0.81, product = 0.6561
        let confidences = [0.81, 0.81];
        let score = score_path(&confidences, EdgeQuality::SingleProvider, 0.5);

        // geo_mean = 0.81, quality_bonus = 1.0, length_bias = 1.0 (neutral)
        let expected = 0.81;
        assert!(
            (score - expected).abs() < 0.01,
            "score {} should be close to {}",
            score,
            expected
        );
    }

    #[tokio::test]
    async fn test_scoring_length_bias() {
        let confidences = [0.9, 0.9, 0.9]; // 3 edges

        // At graduate lens (1.0), length 3 should get a bonus
        let score_grad = score_path(&confidences, EdgeQuality::SingleProvider, 1.0);
        // At 5th grade lens (0.15), length 3 should get a penalty
        let score_5th = score_path(&confidences, EdgeQuality::SingleProvider, 0.15);

        assert!(
            score_grad > score_5th,
            "graduate should score higher for longer path: {} vs {}",
            score_grad,
            score_5th
        );
    }
}
