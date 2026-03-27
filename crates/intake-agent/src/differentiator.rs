use std::collections::{HashMap, HashSet};

use graph_service::graph::KnowledgeGraph;
use graph_service::types::{EdgeType, NodeType};

use crate::candidates::Candidate;

// ---------------------------------------------------------------------------
// Differentiator computation — finds questions that split the candidate set.
//
// Depth-aware differentiation (Gemini + Grok consensus):
//   1. For each candidate, collect all systems it acts_on
//   2. Find systems NOT shared by all candidates → these are discriminating
//   3. If candidates share ALL systems, walk deeper: compare Mechanisms
//   4. If they share Mechanisms, compare Pathways/Substrates
//   5. Walk down until divergence found
//
// Entropy-reduction sort (Grok): prefer questions that split closest to 50/50.
// ---------------------------------------------------------------------------

/// A question that would help distinguish between candidates.
#[derive(Debug, Clone)]
pub struct Differentiator {
    /// Topic of the distinguishing question (e.g., "neurological symptoms")
    pub question_topic: String,
    /// Node type where the divergence was found
    pub divergence_type: NodeType,
    /// Ingredients this question would support if answered positively
    pub favors: Vec<String>,
    /// Ingredients this question would weaken if answered positively
    pub disfavors: Vec<String>,
    /// Graph basis explanation (e.g., "magnesium acts_on nervous system; calcium does not")
    pub graph_basis: String,
    /// How evenly this splits the candidate set (1.0 = perfect 50/50)
    pub entropy_score: f64,
}

/// Compute differentiators between the current candidate set.
///
/// Walks the graph from each candidate's ingredient node to find systems,
/// mechanisms, and deeper nodes that are NOT shared across all candidates.
pub async fn compute_differentiators(
    candidates: &[Candidate],
    graph: &KnowledgeGraph,
    systems_already_reviewed: &HashSet<String>,
) -> Vec<Differentiator> {
    if candidates.len() < 2 {
        return Vec::new();
    }

    let mut differentiators = Vec::new();
    let total = candidates.len();

    // Level 1: Systems each candidate acts_on
    let mut ingredient_systems: HashMap<String, HashSet<String>> = HashMap::new();
    for candidate in candidates {
        let systems = collect_targets(graph, &candidate.ingredient, &EdgeType::ActsOn).await;
        ingredient_systems.insert(candidate.ingredient.clone(), systems);
    }

    differentiators.extend(find_divergences(
        &ingredient_systems,
        total,
        NodeType::System,
        "acts_on",
        systems_already_reviewed,
    ));

    // Level 2: Mechanisms each candidate uses (via_mechanism)
    if differentiators.is_empty() {
        let mut ingredient_mechanisms: HashMap<String, HashSet<String>> = HashMap::new();
        for candidate in candidates {
            let mechs =
                collect_targets(graph, &candidate.ingredient, &EdgeType::ViaMechanism).await;
            ingredient_mechanisms.insert(candidate.ingredient.clone(), mechs);
        }

        differentiators.extend(find_divergences(
            &ingredient_mechanisms,
            total,
            NodeType::Mechanism,
            "via_mechanism",
            &HashSet::new(), // mechanisms aren't "reviewed" in ROS
        ));
    }

    // Level 3: Properties each candidate affords
    if differentiators.is_empty() {
        let mut ingredient_properties: HashMap<String, HashSet<String>> = HashMap::new();
        for candidate in candidates {
            let props = collect_targets(graph, &candidate.ingredient, &EdgeType::Affords).await;
            ingredient_properties.insert(candidate.ingredient.clone(), props);
        }

        differentiators.extend(find_divergences(
            &ingredient_properties,
            total,
            NodeType::Property,
            "affords",
            &HashSet::new(),
        ));
    }

    // Sort by entropy score (best splits first)
    differentiators.sort_by(|a, b| b.entropy_score.partial_cmp(&a.entropy_score).unwrap());

    differentiators
}

/// Collect target node names for a given ingredient and edge type.
async fn collect_targets(
    graph: &KnowledgeGraph,
    ingredient_name: &str,
    edge_type: &EdgeType,
) -> HashSet<String> {
    let mut targets = HashSet::new();
    if let Some(idx) = graph.find_node(ingredient_name).await {
        for (target_idx, edge_data) in graph.outgoing_edges(&idx).await {
            if edge_data.edge_type == *edge_type {
                if let Some(target_data) = graph.node_data(&target_idx).await {
                    targets.insert(target_data.name.to_lowercase());
                }
            }
        }
    }
    targets
}

/// Find nodes that are NOT shared by all candidates → differentiating questions.
fn find_divergences(
    ingredient_targets: &HashMap<String, HashSet<String>>,
    total_candidates: usize,
    node_type: NodeType,
    edge_label: &str,
    already_reviewed: &HashSet<String>,
) -> Vec<Differentiator> {
    // Count how many candidates connect to each target
    let mut target_counts: HashMap<&str, Vec<&str>> = HashMap::new();
    for (ingredient, targets) in ingredient_targets {
        for target in targets {
            target_counts
                .entry(target.as_str())
                .or_default()
                .push(ingredient.as_str());
        }
    }

    let mut diffs = Vec::new();
    for (target, favoring_ingredients) in &target_counts {
        // Skip if ALL candidates share this target (not differentiating)
        if favoring_ingredients.len() == total_candidates {
            continue;
        }
        // Skip already-reviewed systems
        if already_reviewed.contains(*target) {
            continue;
        }

        let favors: Vec<String> = favoring_ingredients.iter().map(|s| s.to_string()).collect();
        let disfavors: Vec<String> = ingredient_targets
            .keys()
            .filter(|k| !favoring_ingredients.contains(&k.as_str()))
            .cloned()
            .collect();

        // Entropy score: how close to 50/50 does this split?
        let favor_fraction = favoring_ingredients.len() as f64 / total_candidates as f64;
        let entropy = 1.0 - (favor_fraction - 0.5).abs() * 2.0; // 1.0 at 50/50, 0.0 at 100/0

        let graph_basis = format!(
            "{} {} {}; {} do not",
            favors.join(", "),
            edge_label,
            target,
            disfavors.join(", ")
        );

        diffs.push(Differentiator {
            question_topic: target.to_string(),
            divergence_type: node_type.clone(),
            favors,
            disfavors,
            graph_basis,
            entropy_score: entropy,
        });
    }
    diffs
}

/// Systems adjacent to current candidates that haven't been reviewed yet.
/// Used during ReviewOfSystems phase to pick which systems to ask about.
pub async fn unreviewed_systems(
    candidates: &[Candidate],
    graph: &KnowledgeGraph,
    systems_reviewed: &HashSet<String>,
) -> Vec<String> {
    let mut all_systems = HashSet::new();
    for candidate in candidates {
        let systems = collect_targets(graph, &candidate.ingredient, &EdgeType::ActsOn).await;
        all_systems.extend(systems);
    }

    let mut unreviewed: Vec<String> = all_systems
        .difference(systems_reviewed)
        .cloned()
        .collect();
    unreviewed.sort();
    unreviewed
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entropy_score_perfect_split() {
        // 1 out of 2 candidates → perfect 50/50
        let favor_fraction: f64 = 1.0 / 2.0;
        let entropy: f64 = 1.0 - (favor_fraction - 0.5).abs() * 2.0;
        assert!((entropy - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_entropy_score_no_split() {
        // 2 out of 2 candidates → no split
        let favor_fraction: f64 = 2.0 / 2.0;
        let entropy: f64 = 1.0 - (favor_fraction - 0.5).abs() * 2.0;
        assert!((entropy - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_find_divergences_basic() {
        let mut targets = HashMap::new();
        let mut mag_targets = HashSet::new();
        mag_targets.insert("nervous system".to_string());
        mag_targets.insert("muscular system".to_string());

        let mut cal_targets = HashSet::new();
        cal_targets.insert("muscular system".to_string());
        cal_targets.insert("skeletal system".to_string());

        targets.insert("magnesium".to_string(), mag_targets);
        targets.insert("calcium".to_string(), cal_targets);

        let diffs = find_divergences(&targets, 2, NodeType::System, "acts_on", &HashSet::new());

        // "muscular system" is shared → not a differentiator
        // "nervous system" favors magnesium, "skeletal system" favors calcium
        assert_eq!(diffs.len(), 2);

        let topics: HashSet<&str> = diffs.iter().map(|d| d.question_topic.as_str()).collect();
        assert!(topics.contains("nervous system"));
        assert!(topics.contains("skeletal system"));
    }

    #[test]
    fn test_find_divergences_skips_reviewed() {
        let mut targets = HashMap::new();
        let mut mag_targets = HashSet::new();
        mag_targets.insert("nervous system".to_string());
        targets.insert("magnesium".to_string(), mag_targets);

        let mut cal_targets = HashSet::new();
        cal_targets.insert("muscular system".to_string());
        targets.insert("calcium".to_string(), cal_targets);

        let mut reviewed = HashSet::new();
        reviewed.insert("nervous system".to_string());

        let diffs = find_divergences(&targets, 2, NodeType::System, "acts_on", &reviewed);

        // "nervous system" is already reviewed → skipped
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].question_topic, "muscular system");
    }
}
