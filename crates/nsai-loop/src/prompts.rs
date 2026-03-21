use crate::analyzer::{Gap, GapKind};

// ---------------------------------------------------------------------------
// Gap-filling question generation
//
// Given a gap in the graph, generate a targeted question at the current
// grade level to fill it.
// ---------------------------------------------------------------------------

const AUDIENCE_5TH: &str = "a 5th grader (10 years old). Use simple everyday words. \
    No scientific terms. Focus on what it does to the body that a kid could understand";

/// Generate a gap-filling question for a specific gap at 5th grade level.
pub fn gap_question(nutraceutical: &str, gap: &Gap) -> String {
    match &gap.kind {
        GapKind::LeafNode => {
            format!(
                "Explain to {} why {} is connected to {}, in one sentence.",
                AUDIENCE_5TH, nutraceutical, gap.node_name
            )
        }
        GapKind::NoMechanism => {
            format!(
                "Explain to {} how {} helps with {}, in one sentence.",
                AUDIENCE_5TH, nutraceutical, gap.node_name
            )
        }
        GapKind::IndirectSystem => {
            format!(
                "Explain to {} what {} does to the {}, in one sentence.",
                AUDIENCE_5TH, nutraceutical, gap.node_name
            )
        }
    }
}

/// System prompt for gap-filling questions (same constraints as curriculum)
pub fn gap_system_prompt() -> &'static str {
    "You are a nutraceutical knowledge extraction assistant.\n\
     \n\
     Rules:\n\
     - Answer in exactly ONE sentence. Be concise but specific.\n\
     - Do not discuss diseases or diagnoses. Frame everything in terms of \
       symptoms and physiological function.\n\
     - Do not speculate beyond well-established knowledge.\n\
     - Match your vocabulary to the audience level specified in the question.\n\
     - Do not hedge with unnecessary qualifiers. Be direct."
}

// ---------------------------------------------------------------------------
// Comprehension check prompt
//
// Ask the LLM to rephrase what has been learned so far, using different
// words. If re-extraction produces the same graph structure, understanding
// is solid. If it diverges, something is shallow or wrong.
// ---------------------------------------------------------------------------

/// Build a comprehension prompt that summarizes current graph knowledge
/// and asks the LLM to rephrase it.
pub fn comprehension_prompt(nutraceutical: &str, knowledge_summary: &str) -> String {
    format!(
        "Here is what we know about {} as a supplement, explained for a 5th grader:\n\
         \n\
         {}\n\
         \n\
         Now explain the same information back in your own words, using completely different \
         words and phrasing, as if you were a different 5th grader explaining it to a friend. \
         Use one sentence per fact. Do not add any new information — only rephrase what is above.",
        nutraceutical, knowledge_summary
    )
}

/// System prompt for the comprehension check
pub fn comprehension_system_prompt() -> &'static str {
    "You are a student rephrasing what you learned about a supplement.\n\
     \n\
     Rules:\n\
     - Use simple everyday words a 5th grader would use.\n\
     - Rephrase each fact using DIFFERENT words than the original.\n\
     - Do not add new information. Only rephrase what was given.\n\
     - One sentence per fact.\n\
     - Do not discuss diseases or diagnoses."
}

/// Build a plain-English summary of the current graph for the comprehension prompt.
/// e.g. "Magnesium helps with muscle relaxation. Magnesium works on the muscular system."
pub fn summarize_graph_for_comprehension(
    graph: &graph_service::graph::KnowledgeGraph,
    nutraceutical: &str,
) -> String {
    let ingredient_name = nutraceutical.to_lowercase();
    let mut sentences = Vec::new();

    if let Some(idx) = graph.find_node(&ingredient_name) {
        for (target_idx, edge_data) in graph.outgoing_edges(idx) {
            if let Some(target) = graph.node_data(target_idx) {
                let sentence = match &edge_data.edge_type {
                    graph_service::types::EdgeType::Affords => {
                        format!("{} helps with {}.", nutraceutical, target.name)
                    }
                    graph_service::types::EdgeType::ActsOn => {
                        format!("{} works on the {}.", nutraceutical, target.name)
                    }
                    graph_service::types::EdgeType::ViaMechanism => {
                        format!("{} works through {}.", nutraceutical, target.name)
                    }
                    graph_service::types::EdgeType::Modulates => {
                        format!("{} affects {}.", nutraceutical, target.name)
                    }
                    _ => continue,
                };
                sentences.push(sentence);
            }
        }
    }

    sentences.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{Gap, GapKind};
    use graph_service::graph::KnowledgeGraph;
    use graph_service::types::*;

    #[test]
    fn test_gap_question_leaf_node() {
        let mut graph = KnowledgeGraph::new();
        let idx = graph.add_node(NodeData::new("muscle relaxation", NodeType::Property));
        let gap = Gap {
            node_idx: idx,
            node_name: "muscle relaxation".to_string(),
            kind: GapKind::LeafNode,
        };

        let q = gap_question("Magnesium", &gap);
        assert!(q.contains("5th grader"));
        assert!(q.contains("Magnesium"));
        assert!(q.contains("muscle relaxation"));
    }

    #[test]
    fn test_gap_question_no_mechanism() {
        let mut graph = KnowledgeGraph::new();
        let idx = graph.add_node(NodeData::new("sleep quality", NodeType::Property));
        let gap = Gap {
            node_idx: idx,
            node_name: "sleep quality".to_string(),
            kind: GapKind::NoMechanism,
        };

        let q = gap_question("Magnesium", &gap);
        assert!(q.contains("how"));
        assert!(q.contains("sleep quality"));
    }

    #[test]
    fn test_summarize_graph() {
        let mut graph = KnowledgeGraph::new();
        let mag = graph.add_node(NodeData::new("magnesium", NodeType::Ingredient));
        let prop = graph.add_node(NodeData::new("muscle relaxation", NodeType::Property));
        let sys = graph.add_node(NodeData::new("muscular system", NodeType::System));

        graph.add_edge(
            mag,
            prop,
            EdgeData::new(EdgeType::Affords, EdgeMetadata::extracted(0.7, 1, 0)),
        );
        graph.add_edge(
            mag,
            sys,
            EdgeData::new(EdgeType::ActsOn, EdgeMetadata::extracted(0.7, 1, 0)),
        );

        let summary = summarize_graph_for_comprehension(&graph, "Magnesium");
        assert!(summary.contains("Magnesium helps with muscle relaxation"));
        assert!(summary.contains("Magnesium works on the muscular system"));
    }

    #[test]
    fn test_comprehension_prompt_includes_summary() {
        let p = comprehension_prompt("Magnesium", "Magnesium helps with muscle relaxation.");
        assert!(p.contains("Magnesium"));
        assert!(p.contains("muscle relaxation"));
        assert!(p.contains("different words"));
    }
}
