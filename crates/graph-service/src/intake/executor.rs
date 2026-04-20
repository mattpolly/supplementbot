// ---------------------------------------------------------------------------
// GraphAction Executor
//
// Dispatches supplement KG queries triggered by the intake traversal engine.
// This is the bridge between the two graphs: the intake KG says "do X",
// the executor runs X against the supplement KG and returns structured results.
// ---------------------------------------------------------------------------

use crate::graph::KnowledgeGraph;
use crate::merge::MergeStore;
use crate::query::{QueryConfig, QueryEngine};
use crate::source::SourceStore;

use super::idisk::IdiskImporter;
use super::types::GraphActionType;

/// Results from executing graph actions for a single turn.
#[derive(Debug, Default)]
pub struct ActionResults {
    /// Candidate ingredient names with scores.
    pub candidates: Vec<CandidateResult>,
    /// Differentiating question topics between top candidates.
    pub discriminators: Vec<DiscriminatorResult>,
    /// Drug interaction warnings.
    pub interactions: Vec<InteractionResult>,
    /// Adverse reaction matches (user's symptoms that could be caused by their supplements).
    pub adverse_matches: Vec<AdverseMatchResult>,
    /// Mechanism of Action text for top candidates.
    pub mechanisms: Vec<MechanismResult>,
    /// Adjacent body systems for system review.
    pub adjacent_systems: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CandidateResult {
    pub ingredient: String,
    pub score: f64,
    pub quality: String,
    pub path_explanations: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DiscriminatorResult {
    pub question_topic: String,
    pub favors: Vec<String>,
    pub entropy: f64,
    pub basis: String,
}

#[derive(Debug, Clone)]
pub struct InteractionResult {
    pub ingredient: String,
    pub drug: String,
    pub description: Option<String>,
    pub rating: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AdverseMatchResult {
    pub ingredient: String,
    pub symptom: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct MechanismResult {
    pub ingredient: String,
    pub mechanism_text: String,
}

/// Executes graph actions against the supplement KG and iDISK data.
pub struct GraphActionExecutor<'a> {
    graph: &'a KnowledgeGraph,
    source: &'a SourceStore,
    merge: &'a MergeStore,
    idisk: &'a IdiskImporter,
}

impl<'a> GraphActionExecutor<'a> {
    pub fn new(
        graph: &'a KnowledgeGraph,
        source: &'a SourceStore,
        merge: &'a MergeStore,
        idisk: &'a IdiskImporter,
    ) -> Self {
        Self {
            graph,
            source,
            merge,
            idisk,
        }
    }

    /// Execute a set of graph actions and return combined results.
    ///
    /// `systems` supplements `symptoms` for candidate lookup: when a user's
    /// complaint maps to a body system but not a specific symptom node (e.g.
    /// "nasal congestion" → immune system), system-level queries still surface
    /// the right ingredients.
    pub async fn execute(
        &self,
        actions: &[GraphActionType],
        symptoms: &[String],
        systems: &[String],
        candidate_names: &[String],
        disclosed_medications: &[String],
        disclosed_supplements: &[String],
        lens_level: f64,
    ) -> ActionResults {
        let mut results = ActionResults::default();

        for action in actions {
            match action {
                GraphActionType::QueryCandidates => {
                    self.query_candidates(symptoms, systems, lens_level, &mut results)
                        .await;
                }
                GraphActionType::FindDiscriminators => {
                    self.find_discriminators(candidate_names, &mut results)
                        .await;
                }
                GraphActionType::CheckInteractions => {
                    self.check_interactions(
                        candidate_names,
                        disclosed_medications,
                        &mut results,
                    )
                    .await;
                }
                GraphActionType::CheckAdverseReactions => {
                    self.check_adverse_reactions(
                        symptoms,
                        disclosed_supplements,
                        &mut results,
                    )
                    .await;
                }
                GraphActionType::FetchMechanism => {
                    self.fetch_mechanisms(candidate_names, &mut results).await;
                }
                GraphActionType::FindAdjacentSystems => {
                    self.find_adjacent_systems(candidate_names, &mut results)
                        .await;
                }
            }
        }

        results
    }

    // -----------------------------------------------------------------------
    // Action implementations
    // -----------------------------------------------------------------------

    async fn query_candidates(
        &self,
        symptoms: &[String],
        systems: &[String],
        lens_level: f64,
        results: &mut ActionResults,
    ) {
        let qe = QueryEngine::new(self.graph, self.source, self.merge).await;
        let config = QueryConfig {
            lens: crate::lens::ComplexityLens::new(lens_level),
            max_depth: 4,
            max_paths_per_result: 3,
            // Require at least one real source before a candidate can be recommended.
            // Deduced and Speculative edges still guide questions but won't produce
            // candidates weak enough to embarrass the recommendation.
            min_quality: Some(crate::source::EdgeQuality::SingleProvider),
            ..Default::default()
        };

        // Helper closure to merge a recommendation result into results.candidates
        let mut merge_rec = |rec: crate::query::RecommendationResult| {
            if let Some(existing) = results
                .candidates
                .iter_mut()
                .find(|c| c.ingredient == rec.ingredient.name)
            {
                if rec.best_score > existing.score {
                    existing.score = rec.best_score;
                }
            } else {
                let explanations: Vec<String> = rec
                    .paths
                    .iter()
                    .flat_map(|p| p.explanation.iter().cloned())
                    .collect();
                results.candidates.push(CandidateResult {
                    ingredient: rec.ingredient.name.clone(),
                    score: rec.best_score,
                    quality: format!("{:?}", rec.weakest_quality),
                    path_explanations: explanations,
                });
            }
        };

        for symptom in symptoms {
            for rec in qe.ingredients_for_symptom(symptom, &config).await {
                merge_rec(rec);
            }
        }

        // When symptom nodes don't exist in the graph (e.g. "nasal congestion"
        // has no node but maps to "immune system" via the allergy profile),
        // query by system so we still surface relevant candidates.
        for system in systems {
            for rec in qe.ingredients_for_system(system, &config).await {
                merge_rec(rec);
            }
        }

        // Sort by score
        results
            .candidates
            .sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    }

    async fn find_discriminators(
        &self,
        candidate_names: &[String],
        results: &mut ActionResults,
    ) {
        if candidate_names.len() < 2 {
            return;
        }

        // Use the existing differentiator from intake-agent's logic
        // but via the graph directly. Walk outgoing edges from each
        // candidate to find non-shared targets.
        let mut per_ingredient_systems: Vec<(String, Vec<String>)> = Vec::new();

        for name in candidate_names.iter().take(5) {
            if let Some(idx) = self.graph.find_node(name).await {
                let edges = self.graph.outgoing_edges(&idx).await;
                let systems: Vec<String> = edges
                    .iter()
                    .filter(|(_, ed)| {
                        ed.edge_type == crate::types::EdgeType::ActsOn
                    })
                    .filter_map(|(target, _)| {
                        // We'd need to get the name — simplify for now
                        Some(format!("{:?}", target.id()))
                    })
                    .collect();
                per_ingredient_systems.push((name.clone(), systems));
            }
        }

        // Find systems unique to subsets of candidates
        if per_ingredient_systems.len() >= 2 {
            let all_systems: std::collections::HashSet<String> = per_ingredient_systems
                .iter()
                .flat_map(|(_, systems)| systems.iter().cloned())
                .collect();

            for system in all_systems {
                let has_it: Vec<String> = per_ingredient_systems
                    .iter()
                    .filter(|(_, systems)| systems.contains(&system))
                    .map(|(name, _)| name.clone())
                    .collect();

                // Only discriminating if not shared by all
                if !has_it.is_empty() && has_it.len() < per_ingredient_systems.len() {
                    let favor_fraction =
                        has_it.len() as f64 / per_ingredient_systems.len() as f64;
                    let entropy = 1.0 - (favor_fraction - 0.5_f64).abs() * 2.0;

                    results.discriminators.push(DiscriminatorResult {
                        question_topic: system.clone(),
                        favors: has_it,
                        entropy,
                        basis: format!("System divergence: {}", system),
                    });
                }
            }

            results
                .discriminators
                .sort_by(|a, b| b.entropy.partial_cmp(&a.entropy).unwrap_or(std::cmp::Ordering::Equal));
        }
    }

    async fn check_interactions(
        &self,
        candidate_names: &[String],
        medications: &[String],
        results: &mut ActionResults,
    ) {
        for med in medications {
            let interactions = self
                .idisk
                .interactions_with_drug(candidate_names, med)
                .await;
            for (ingredient, interaction) in interactions {
                results.interactions.push(InteractionResult {
                    ingredient,
                    drug: med.clone(),
                    description: interaction.description,
                    rating: interaction.rating,
                });
            }
        }
    }

    async fn check_adverse_reactions(
        &self,
        symptoms: &[String],
        supplements: &[String],
        results: &mut ActionResults,
    ) {
        for supp in supplements {
            let reactions = self.idisk.adverse_reactions_for(supp).await;
            for (symptom_id, source) in reactions {
                // Check if any of the user's symptoms match this adverse reaction
                let symptom_lower = symptom_id.replace('_', " ");
                for user_symptom in symptoms {
                    if user_symptom.to_lowercase().contains(&symptom_lower)
                        || symptom_lower.contains(&user_symptom.to_lowercase())
                    {
                        results.adverse_matches.push(AdverseMatchResult {
                            ingredient: supp.clone(),
                            symptom: symptom_id.clone(),
                            source,
                        });
                        break;
                    }
                }
            }
        }
    }

    async fn fetch_mechanisms(
        &self,
        candidate_names: &[String],
        results: &mut ActionResults,
    ) {
        for name in candidate_names.iter().take(5) {
            if let Some(text) = self.idisk.mechanism_of_action(name).await {
                // Truncate to reasonable length for prompt inclusion
                let truncated = if text.len() > 500 {
                    format!("{}...", &text[..497])
                } else {
                    text
                };
                results.mechanisms.push(MechanismResult {
                    ingredient: name.clone(),
                    mechanism_text: truncated,
                });
            }
        }
    }

    async fn find_adjacent_systems(
        &self,
        candidate_names: &[String],
        results: &mut ActionResults,
    ) {
        let mut systems = std::collections::HashSet::new();

        for name in candidate_names {
            if let Some(idx) = self.graph.find_node(name).await {
                let edges = self.graph.outgoing_edges(&idx).await;
                for (target, ed) in edges {
                    if ed.edge_type == crate::types::EdgeType::ActsOn {
                        if let Some(data) = self.graph.node_data(&target).await {
                            systems.insert(data.name);
                        }
                    }
                }
            }
        }

        results.adjacent_systems = systems.into_iter().collect();
        results.adjacent_systems.sort();
    }
}
