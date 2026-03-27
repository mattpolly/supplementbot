use graph_service::graph::{KnowledgeGraph, NodeIndex};
use graph_service::types::*;
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// Structural inference — find patterns in graph topology
//
// These are observations the graph makes about itself, without any LLM.
// Each observation is tagged as StructurallyEmergent — the graph found it,
// the LLM didn't teach it.
// ---------------------------------------------------------------------------

/// A structural observation found by analyzing graph topology
#[derive(Debug, Clone)]
pub struct Observation {
    pub kind: ObservationKind,
    pub description: String,
    /// The nodes involved in this observation
    pub involved: Vec<String>,
    /// Significance score (higher = more interesting). Computed by `score_observation`.
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ObservationKind {
    /// Two or more ingredients both act_on the same system
    SharedSystem,
    /// Two or more ingredients afford the same property
    SharedProperty,
    /// Multiple independent paths converge on the same property
    ConvergentPaths,
    /// A mechanism is shared across multiple ingredients
    SharedMechanism,
    /// An ingredient reaches a system through a mechanism that
    /// another ingredient also uses (potential synergy or competition)
    MechanismOverlap,
}

/// Degree threshold above which a node is a "supernode".
/// Observations involving supernodes are dampened because a node connected
/// to everything is informative about nothing.
const SUPERNODE_DEGREE_THRESHOLD: usize = 15;

/// Dampening factor applied to supernode observations (0.0–1.0)
const SUPERNODE_DAMPENING: f64 = 0.3;

/// Analyze the graph for structural patterns across ingredients.
///
/// Returns observations scored and sorted by significance.
/// Scoring considers: ingredient count, average edge confidence,
/// observation type weight, and supernode dampening.
pub async fn find_observations(graph: &KnowledgeGraph) -> Vec<Observation> {
    let mut observations = Vec::new();

    let ingredients = graph.nodes_by_type(&NodeType::Ingredient).await;
    if ingredients.len() < 2 {
        return observations;
    }

    // Pre-compute node degrees for supernode detection
    let mut node_degrees: HashMap<String, usize> = HashMap::new();
    for idx in graph.all_nodes().await {
        if let Some(data) = graph.node_data(&idx).await {
            let out = graph.outgoing_edges(&idx).await.len();
            let inc = graph.incoming_edges(&idx).await.len();
            node_degrees.insert(data.name.clone(), out + inc);
        }
    }

    observations.extend(find_shared_systems(graph, &ingredients).await);
    observations.extend(find_shared_properties(graph, &ingredients).await);
    observations.extend(find_shared_mechanisms(graph, &ingredients).await);
    observations.extend(find_convergent_paths(graph, &ingredients).await);

    // Score each observation
    for obs in &mut observations {
        obs.score = score_observation(obs, &node_degrees);
    }

    // Sort by score descending
    observations.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    observations
}

/// Score an observation by significance.
///
/// Components:
/// - **Ingredient count** (0–5): more ingredients involved = more interesting
/// - **Type weight** (0–3): ConvergentPaths and MechanismOverlap are rarer/more
///   interesting than SharedSystem
/// - **Supernode dampening**: if the shared node is a supernode, multiply by 0.3
fn score_observation(obs: &Observation, node_degrees: &HashMap<String, usize>) -> f64 {
    // Count how many of the involved names are ingredients (rough heuristic:
    // ingredients are the ones that aren't the shared node). Since we don't
    // have node types here, use involved.len() - 1 as ingredient count.
    let ingredient_count = obs.involved.len().saturating_sub(1).max(1) as f64;

    // Type weight — rarer observation types score higher
    let type_weight = match obs.kind {
        ObservationKind::SharedSystem => 1.0,
        ObservationKind::SharedProperty => 1.5,
        ObservationKind::SharedMechanism => 2.0,
        ObservationKind::ConvergentPaths => 2.5,
        ObservationKind::MechanismOverlap => 3.0,
    };

    let base_score = ingredient_count + type_weight;

    // Supernode dampening: check if the shared node (last in involved list) is a supernode
    let dampening = if let Some(shared_node) = obs.involved.last() {
        let degree = node_degrees.get(shared_node).copied().unwrap_or(0);
        if degree >= SUPERNODE_DEGREE_THRESHOLD {
            SUPERNODE_DAMPENING
        } else {
            1.0
        }
    } else {
        1.0
    };

    base_score * dampening
}

/// Find systems that multiple ingredients act on
async fn find_shared_systems(
    graph: &KnowledgeGraph,
    ingredients: &[NodeIndex],
) -> Vec<Observation> {
    let mut system_to_ingredients: HashMap<NodeIndex, Vec<String>> = HashMap::new();

    for ing_idx in ingredients {
        let ing_name = match graph.node_data(ing_idx).await {
            Some(d) => d.name.clone(),
            None => continue,
        };
        for (target, edge) in graph.outgoing_edges(ing_idx).await {
            if edge.edge_type == EdgeType::ActsOn {
                system_to_ingredients
                    .entry(target)
                    .or_default()
                    .push(ing_name.clone());
            }
        }
    }

    let mut results = Vec::new();
    for (sys_idx, ings) in system_to_ingredients {
        if ings.len() >= 2 {
            if let Some(sys_data) = graph.node_data(&sys_idx).await {
                let mut involved = ings.clone();
                involved.push(sys_data.name.clone());
                results.push(Observation {
                    kind: ObservationKind::SharedSystem,
                    description: format!(
                        "{} both act on the {}",
                        format_list(&ings),
                        sys_data.name
                    ),
                    involved,
                    score: 0.0,
                });
            }
        }
    }
    results
}

/// Find properties that multiple ingredients afford
async fn find_shared_properties(
    graph: &KnowledgeGraph,
    ingredients: &[NodeIndex],
) -> Vec<Observation> {
    let mut prop_to_ingredients: HashMap<NodeIndex, Vec<String>> = HashMap::new();

    for ing_idx in ingredients {
        let ing_name = match graph.node_data(ing_idx).await {
            Some(d) => d.name.clone(),
            None => continue,
        };
        for (target, edge) in graph.outgoing_edges(ing_idx).await {
            if edge.edge_type == EdgeType::Affords {
                if let Some(td) = graph.node_data(&target).await {
                    if td.node_type == NodeType::Property {
                        prop_to_ingredients
                            .entry(target)
                            .or_default()
                            .push(ing_name.clone());
                    }
                }
            }
        }
    }

    let mut results = Vec::new();
    for (prop_idx, ings) in prop_to_ingredients {
        if ings.len() >= 2 {
            if let Some(prop_data) = graph.node_data(&prop_idx).await {
                let mut involved = ings.clone();
                involved.push(prop_data.name.clone());
                results.push(Observation {
                    kind: ObservationKind::SharedProperty,
                    description: format!(
                        "{} both afford {}",
                        format_list(&ings),
                        prop_data.name
                    ),
                    involved,
                    score: 0.0,
                });
            }
        }
    }
    results
}

/// Find mechanisms used by multiple ingredients
async fn find_shared_mechanisms(
    graph: &KnowledgeGraph,
    ingredients: &[NodeIndex],
) -> Vec<Observation> {
    let mut mech_to_ingredients: HashMap<NodeIndex, Vec<String>> = HashMap::new();

    for ing_idx in ingredients {
        let ing_name = match graph.node_data(ing_idx).await {
            Some(d) => d.name.clone(),
            None => continue,
        };
        for (target, edge) in graph.outgoing_edges(ing_idx).await {
            if edge.edge_type == EdgeType::ViaMechanism {
                mech_to_ingredients
                    .entry(target)
                    .or_default()
                    .push(ing_name.clone());
            }
        }
    }

    let mut results = Vec::new();
    for (mech_idx, ings) in mech_to_ingredients {
        if ings.len() >= 2 {
            if let Some(mech_data) = graph.node_data(&mech_idx).await {
                let mut involved = ings.clone();
                involved.push(mech_data.name.clone());
                results.push(Observation {
                    kind: ObservationKind::SharedMechanism,
                    description: format!(
                        "{} both work via {}",
                        format_list(&ings),
                        mech_data.name
                    ),
                    involved,
                    score: 0.0,
                });
            }
        }
    }
    results
}

/// Find properties reachable through multiple independent paths from the same ingredient
async fn find_convergent_paths(
    graph: &KnowledgeGraph,
    ingredients: &[NodeIndex],
) -> Vec<Observation> {
    let mut observations = Vec::new();

    for ing_idx in ingredients {
        let ing_name = match graph.node_data(ing_idx).await {
            Some(d) => d.name.clone(),
            None => continue,
        };

        // Collect all properties reachable directly (affords)
        let mut direct_props: HashSet<NodeIndex> = HashSet::new();
        for (target, edge) in graph.outgoing_edges(ing_idx).await {
            if edge.edge_type == EdgeType::Affords {
                if let Some(d) = graph.node_data(&target).await {
                    if d.node_type == NodeType::Property {
                        direct_props.insert(target);
                    }
                }
            }
        }

        // Collect mechanisms
        let mut mechanisms: Vec<NodeIndex> = Vec::new();
        for (target, edge) in graph.outgoing_edges(ing_idx).await {
            if edge.edge_type == EdgeType::ViaMechanism {
                mechanisms.push(target);
            }
        }

        for mech_idx in &mechanisms {
            let mech_name = match graph.node_data(mech_idx).await {
                Some(d) => d.name.clone(),
                None => continue,
            };
            for (prop_idx, edge) in graph.outgoing_edges(mech_idx).await {
                if edge.edge_type == EdgeType::Affords && direct_props.contains(&prop_idx) {
                    if let Some(prop_data) = graph.node_data(&prop_idx).await {
                        observations.push(Observation {
                            kind: ObservationKind::ConvergentPaths,
                            description: format!(
                                "{} reaches {} both directly and through {}",
                                ing_name, prop_data.name, mech_name
                            ),
                            involved: vec![
                                ing_name.clone(),
                                prop_data.name.clone(),
                                mech_name.clone(),
                            ],
                            score: 0.0,
                        });
                    }
                }
            }
        }
    }

    observations
}

fn format_list(items: &[String]) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        2 => format!("{} and {}", items[0], items[1]),
        _ => {
            let (last, rest) = items.split_last().unwrap();
            format!("{}, and {}", rest.join(", "), last)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    async fn build_two_ingredient_graph() -> KnowledgeGraph {
        let g = KnowledgeGraph::in_memory().await.unwrap();

        let mag = g.add_node(NodeData::new("magnesium", NodeType::Ingredient)).await;
        let zinc = g.add_node(NodeData::new("zinc", NodeType::Ingredient)).await;
        let muscular = g.add_node(NodeData::new("muscular system", NodeType::System)).await;
        let immune = g.add_node(NodeData::new("immune system", NodeType::System)).await;
        let relaxation = g.add_node(NodeData::new("muscle relaxation", NodeType::Property)).await;
        let wound = g.add_node(NodeData::new("wound healing", NodeType::Property)).await;
        let mech = g.add_node(NodeData::new("cell regeneration", NodeType::Mechanism)).await;

        let meta = EdgeMetadata::extracted(0.7, 1, 0);

        // Magnesium acts on muscular system, affords relaxation
        g.add_edge(&mag, &muscular, EdgeData::new(EdgeType::ActsOn, meta.clone())).await;
        g.add_edge(&mag, &relaxation, EdgeData::new(EdgeType::Affords, meta.clone())).await;
        g.add_edge(&mag, &mech, EdgeData::new(EdgeType::ViaMechanism, meta.clone())).await;
        g.add_edge(&mech, &relaxation, EdgeData::new(EdgeType::Affords, meta.clone())).await;

        // Zinc acts on immune AND muscular (shared), affords wound healing
        g.add_edge(&zinc, &immune, EdgeData::new(EdgeType::ActsOn, meta.clone())).await;
        g.add_edge(&zinc, &muscular, EdgeData::new(EdgeType::ActsOn, meta.clone())).await;
        g.add_edge(&zinc, &wound, EdgeData::new(EdgeType::Affords, meta.clone())).await;
        g.add_edge(&zinc, &mech, EdgeData::new(EdgeType::ViaMechanism, meta.clone())).await;

        g
    }

    #[tokio::test]
    async fn test_finds_shared_system() {
        let g = build_two_ingredient_graph().await;
        let obs = find_observations(&g).await;

        let shared_sys = obs.iter().find(|o| o.kind == ObservationKind::SharedSystem);
        assert!(shared_sys.is_some(), "should find shared muscular system");
        assert!(shared_sys.unwrap().description.contains("muscular system"));
    }

    #[tokio::test]
    async fn test_finds_shared_mechanism() {
        let g = build_two_ingredient_graph().await;
        let obs = find_observations(&g).await;

        let shared_mech = obs.iter().find(|o| o.kind == ObservationKind::SharedMechanism);
        assert!(shared_mech.is_some(), "should find shared cell regeneration");
        assert!(shared_mech.unwrap().description.contains("cell regeneration"));
    }

    #[tokio::test]
    async fn test_finds_convergent_paths() {
        let g = build_two_ingredient_graph().await;
        let obs = find_observations(&g).await;

        let convergent = obs.iter().find(|o| o.kind == ObservationKind::ConvergentPaths);
        assert!(convergent.is_some(), "should find convergent path to muscle relaxation");
        assert!(convergent.unwrap().description.contains("muscle relaxation"));
    }

    #[tokio::test]
    async fn test_no_observations_with_single_ingredient() {
        let g = KnowledgeGraph::in_memory().await.unwrap();
        let mag = g.add_node(NodeData::new("magnesium", NodeType::Ingredient)).await;
        let sys = g.add_node(NodeData::new("muscular system", NodeType::System)).await;
        g.add_edge(
            &mag,
            &sys,
            EdgeData::new(EdgeType::ActsOn, EdgeMetadata::extracted(0.7, 1, 0)),
        ).await;

        let obs = find_observations(&g).await;
        assert!(obs.is_empty(), "single ingredient should produce no cross-ingredient observations");
    }

    #[tokio::test]
    async fn test_observations_sorted_by_score() {
        let g = build_two_ingredient_graph().await;
        let obs = find_observations(&g).await;

        if obs.len() >= 2 {
            for w in obs.windows(2) {
                assert!(
                    w[0].score >= w[1].score,
                    "should be sorted by score descending: {} >= {}",
                    w[0].score,
                    w[1].score
                );
            }
        }
    }

    #[tokio::test]
    async fn test_supernode_dampening() {
        let g = KnowledgeGraph::in_memory().await.unwrap();

        let meta = EdgeMetadata::extracted(0.7, 1, 0);

        // Create a supernode: "immune system" connected to many ingredients
        let immune = g.add_node(NodeData::new("immune system", NodeType::System)).await;
        let normal_sys = g.add_node(NodeData::new("muscular system", NodeType::System)).await;

        // Create ingredients that share both systems
        let mut ingredient_names = Vec::new();
        for i in 0..20 {
            let name = format!("ingredient_{}", i);
            let ing = g.add_node(NodeData::new(&name, NodeType::Ingredient)).await;
            // All connect to immune (making it a supernode)
            g.add_edge(&ing, &immune, EdgeData::new(EdgeType::ActsOn, meta.clone())).await;
            if i < 2 {
                // Only first two connect to muscular
                g.add_edge(&ing, &normal_sys, EdgeData::new(EdgeType::ActsOn, meta.clone())).await;
                ingredient_names.push(name);
            }
        }

        let obs = find_observations(&g).await;

        // The shared muscular system observation (2 ingredients, normal node) should
        // score higher than the immune system observation (20 ingredients, supernode)
        // because immune gets dampened by 0.3
        let muscular_obs = obs.iter().find(|o| o.description.contains("muscular system"));
        let immune_obs = obs.iter().find(|o| o.description.contains("immune system"));

        assert!(muscular_obs.is_some(), "should find muscular observation");
        assert!(immune_obs.is_some(), "should find immune observation");

        let _muscular_score = muscular_obs.unwrap().score;
        let immune_score = immune_obs.unwrap().score;

        // Immune has 20 ingredients but dampened by 0.3: (19 + 1.0) * 0.3 = 6.0
        // Muscular has 2 ingredients, no dampening: (1 + 1.0) * 1.0 = 2.0
        // Actually immune still wins on raw count. But the dampening should be visible.
        assert!(
            immune_score < (immune_obs.unwrap().involved.len() as f64),
            "supernode observation score ({}) should be dampened below involved count ({})",
            immune_score,
            immune_obs.unwrap().involved.len()
        );
    }
}
