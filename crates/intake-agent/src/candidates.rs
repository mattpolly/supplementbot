use std::collections::{HashMap, HashSet};

use graph_service::query::{RecommendationResult, TraversalPath};
use graph_service::source::EdgeQuality;

// ---------------------------------------------------------------------------
// Candidate tracking — maintains a ranked set of ingredient candidates
// with graph evidence.
//
// Three-model consensus scoring (Grok's intersection + coverage approach):
//   1. Intersection gate: candidate must appear in ALL symptom result sets
//   2. Sum per-symptom best scores
//   3. Coverage bonus: score × (1 + 0.3 × coverage_fraction)
//   4. Pertinent negative penalty: reduce score proportional to denied-system paths
//   5. Contraindication elimination
// ---------------------------------------------------------------------------

/// Coverage bonus multiplier — rewards ingredients that cover more symptoms.
const COVERAGE_BONUS: f64 = 0.3;

/// Penalty factor for candidates whose paths go through a denied system.
const NEGATIVE_EVIDENCE_PENALTY: f64 = 0.7;

/// A single ingredient candidate with scoring evidence.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub ingredient: String,
    /// Per-symptom scores from QueryEngine
    pub per_symptom_scores: HashMap<String, f64>,
    /// Final composite score after intersection + coverage + penalties
    pub composite_score: f64,
    /// All supporting traversal paths (from all symptom queries)
    pub supporting_paths: Vec<TraversalPath>,
    /// Weakest quality tier across all paths
    pub quality: EdgeQuality,
    /// Contraindication paths (if any)
    pub contraindications: Vec<TraversalPath>,
}

/// The full ranked candidate set for the current session state.
#[derive(Debug, Default)]
pub struct CandidateSet {
    pub candidates: Vec<Candidate>,
}

impl CandidateSet {
    pub fn new() -> Self {
        Self {
            candidates: Vec::new(),
        }
    }

    /// Top N candidates by composite score.
    pub fn top(&self, n: usize) -> &[Candidate] {
        &self.candidates[..self.candidates.len().min(n)]
    }

    /// Number of active candidates.
    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }
}

/// Score candidates from per-symptom query results.
///
/// `per_symptom_results` is keyed by symptom name → QueryEngine results for that symptom.
/// `denied_systems` is the set of systems the user has explicitly denied.
/// `disclosed_conditions` is for contraindication elimination.
pub fn score_candidates(
    per_symptom_results: &HashMap<String, Vec<RecommendationResult>>,
    denied_systems: &HashSet<String>,
    disclosed_conditions: &[String],
) -> CandidateSet {
    if per_symptom_results.is_empty() {
        return CandidateSet::new();
    }

    let total_symptoms = per_symptom_results.len();
    // Step 1: Collect all ingredients that appear in ANY symptom result
    let mut ingredient_symptoms: HashMap<String, HashMap<String, (f64, Vec<TraversalPath>, EdgeQuality, Vec<TraversalPath>)>> =
        HashMap::new();

    for (symptom, results) in per_symptom_results {
        for rec in results {
            let name = rec.ingredient.name.to_lowercase();
            let entry = ingredient_symptoms.entry(name).or_default();
            entry.insert(
                symptom.clone(),
                (
                    rec.best_score,
                    rec.paths.clone(),
                    rec.weakest_quality,
                    rec.contraindications.clone(),
                ),
            );
        }
    }

    // Step 2: Intersection gate — keep only ingredients present in ALL symptom results
    let mut candidates: Vec<Candidate> = ingredient_symptoms
        .into_iter()
        .filter(|(_, symptom_data)| symptom_data.len() == total_symptoms)
        .map(|(ingredient, symptom_data)| {
            let mut per_symptom_scores = HashMap::new();
            let mut all_paths = Vec::new();
            let mut all_contras = Vec::new();
            let mut weakest = EdgeQuality::CitationBacked;

            for (symptom, (score, paths, quality, contras)) in &symptom_data {
                per_symptom_scores.insert(symptom.clone(), *score);
                all_paths.extend(paths.iter().cloned());
                all_contras.extend(contras.iter().cloned());
                if (*quality as u8) < (weakest as u8) {
                    weakest = *quality;
                }
            }

            // Step 3: Sum of per-symptom best scores × coverage bonus
            let score_sum: f64 = per_symptom_scores.values().sum();
            let coverage_fraction = per_symptom_scores.len() as f64 / total_symptoms as f64;
            let composite = score_sum * (1.0 + COVERAGE_BONUS * coverage_fraction);

            Candidate {
                ingredient,
                per_symptom_scores,
                composite_score: composite,
                supporting_paths: all_paths,
                quality: weakest,
                contraindications: all_contras,
            }
        })
        .collect();

    // Step 4: Pertinent negative penalty — reduce score for candidates whose
    // paths go through denied systems
    if !denied_systems.is_empty() {
        for candidate in &mut candidates {
            let denied_path_count = candidate
                .supporting_paths
                .iter()
                .filter(|path| path_uses_system(path, denied_systems))
                .count();
            if denied_path_count > 0 {
                let total_paths = candidate.supporting_paths.len();
                let denied_fraction = denied_path_count as f64 / total_paths as f64;
                // Penalty proportional to how much evidence goes through denied systems
                candidate.composite_score *=
                    1.0 - (denied_fraction * (1.0 - NEGATIVE_EVIDENCE_PENALTY));
            }
        }
    }

    // Step 5: Contraindication elimination
    if !disclosed_conditions.is_empty() {
        candidates.retain(|c| c.contraindications.is_empty());
    }

    // Sort descending by composite score
    candidates.sort_by(|a, b| b.composite_score.partial_cmp(&a.composite_score).unwrap());

    CandidateSet { candidates }
}

/// Check if a traversal path passes through any of the denied systems.
fn path_uses_system(path: &TraversalPath, denied_systems: &HashSet<String>) -> bool {
    use graph_service::query::PathStep;
    use graph_service::types::NodeType;

    for step in &path.steps {
        if let PathStep::Node { data, .. } = step {
            if data.node_type == NodeType::System
                && denied_systems.contains(&data.name.to_lowercase())
            {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use graph_service::graph::NodeIndex;
    use graph_service::query::{PathStep, TraversalPath};
    use graph_service::types::{NodeData, NodeType};

    fn mock_rec(name: &str, score: f64) -> RecommendationResult {
        RecommendationResult {
            ingredient: NodeData::new(name, NodeType::Ingredient),
            ingredient_index: NodeIndex::default_for_test(),
            paths: vec![TraversalPath {
                steps: vec![PathStep::Node {
                    index: NodeIndex::default_for_test(),
                    data: NodeData::new("muscular system", NodeType::System),
                }],
                score,
                explanation: vec![],
            }],
            best_score: score,
            weakest_quality: EdgeQuality::MultiProvider,
            contraindications: vec![],
        }
    }

    #[test]
    fn test_intersection_gate_filters_partial() {
        let mut per_symptom = HashMap::new();
        per_symptom.insert(
            "muscle cramps".to_string(),
            vec![mock_rec("Magnesium", 0.8), mock_rec("Calcium", 0.6)],
        );
        per_symptom.insert(
            "insomnia".to_string(),
            vec![mock_rec("Magnesium", 0.7)],
            // Calcium absent → filtered out
        );

        let result = score_candidates(&per_symptom, &HashSet::new(), &[]);
        assert_eq!(result.len(), 1);
        assert_eq!(result.candidates[0].ingredient, "magnesium");
    }

    #[test]
    fn test_coverage_bonus_applied() {
        let mut per_symptom = HashMap::new();
        per_symptom.insert(
            "muscle cramps".to_string(),
            vec![mock_rec("Magnesium", 0.8)],
        );

        let result = score_candidates(&per_symptom, &HashSet::new(), &[]);
        assert_eq!(result.len(), 1);
        // score = 0.8 × (1 + 0.3 × 1.0) = 0.8 × 1.3 = 1.04
        assert!((result.candidates[0].composite_score - 1.04).abs() < 0.01);
    }

    #[test]
    fn test_empty_results() {
        let result = score_candidates(&HashMap::new(), &HashSet::new(), &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_denied_system_penalty() {
        let mut per_symptom = HashMap::new();
        per_symptom.insert(
            "muscle cramps".to_string(),
            vec![mock_rec("Magnesium", 0.8)],
        );

        let mut denied = HashSet::new();
        denied.insert("muscular system".to_string());

        let result = score_candidates(&per_symptom, &denied, &[]);
        assert_eq!(result.len(), 1);
        // All paths go through muscular system (denied), so full penalty applies
        // score = 1.04 × (1 - 1.0 × 0.3) = 1.04 × 0.7 = 0.728
        assert!(result.candidates[0].composite_score < 1.04);
    }
}
