// ---------------------------------------------------------------------------
// Structured extraction prompt
//
// Given a one-sentence answer about a nutraceutical, the LLM returns typed
// triples that map onto the graph ontology. The response format is strict:
// one triple per line, pipe-delimited.
//
// The prompt is now lens-aware: only node types and edge types visible at
// the current complexity level are included in the prompt. This prevents
// the LLM from using vocabulary above the current grade level.
// ---------------------------------------------------------------------------

use graph_service::lens::ComplexityLens;
use graph_service::types::{EdgeType, NodeType};

/// Build the full extraction system prompt, filtered by complexity lens.
///
/// If `existing_nodes` is non-empty, the prompt instructs the LLM to reuse
/// those exact names instead of inventing synonyms.
pub fn extraction_system(lens: &ComplexityLens, existing_nodes: &[&str]) -> String {
    let node_types = lens.node_types_prompt();
    let edge_types = lens.edge_types_prompt();

    let vocab_section = if existing_nodes.is_empty() {
        String::new()
    } else {
        format!(
            "\n\
             ## Existing graph nodes\n\
             These nodes already exist in the knowledge graph with their types shown in parentheses. \
             When a concept matches one of these, reuse the EXACT name and type instead of \
             inventing a synonym or using a different type. However, if the sentence contains \
             a genuinely new concept not covered by any existing node, DO create a new node for it.\n\
             {}\n",
            existing_nodes.join(", ")
        )
    };

    format!(
        "You are a knowledge-graph extraction assistant for nutraceutical science.\n\
         \n\
         Given a sentence about a supplement, extract typed relationships as triples.\n\
         \n\
         ## Node types\n\
         {}\n\
         \n\
         ## Edge types\n\
         {}\n\
         {}\
         \n\
         ## Output format\n\
         Return ONLY lines in this exact format, one per line:\n\
         subject_name|subject_type|edge_type|object_name|object_type\n\
         \n\
         Rules:\n\
         - Use lowercase for all names (e.g. \"magnesium\", \"nervous system\")\n\
         - Do not repeat the same triple twice\n\
         - Extract at most 5 triples per sentence\n\
         - Do not include triples you are not confident about\n\
         - Do not include any explanation, headers, or extra text\n\
         - The supplement name should match exactly as given in the prompt\n\
         - ONLY use the node types and edge types listed above. Do not invent new ones.\n\
         - If a concept matches an existing graph node, use that exact name and type. Create new nodes only for genuinely new concepts.\n\
         \n\
         Example input: \"Magnesium helps your muscles relax and helps you sleep better.\"\n\
         Example output:\n\
         magnesium|Ingredient|affords|muscle relaxation|Property\n\
         magnesium|Ingredient|acts_on|muscular system|System\n\
         magnesium|Ingredient|affords|sleep quality|Property",
        node_types, edge_types, vocab_section
    )
}

pub fn extraction_prompt(nutraceutical: &str, sentence: &str) -> String {
    format!(
        "Extract graph triples from this sentence about {}:\n\"{}\"",
        nutraceutical, sentence
    )
}

// ---------------------------------------------------------------------------
// Triple parsing
// ---------------------------------------------------------------------------

/// A parsed triple from the LLM response
#[derive(Debug, Clone, PartialEq)]
pub struct RawTriple {
    pub subject_name: String,
    pub subject_type: NodeType,
    pub edge_type: EdgeType,
    pub object_name: String,
    pub object_type: NodeType,
}

/// Normalize a node name: lowercase, underscores to spaces, collapse whitespace
fn normalize_name(raw: &str) -> String {
    raw.trim()
        .to_lowercase()
        .replace('_', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse a single line into a RawTriple, or return a warning string
fn parse_line(line: &str) -> Result<RawTriple, String> {
    let parts: Vec<&str> = line.split('|').collect();
    if parts.len() != 5 {
        return Err(format!(
            "expected 5 pipe-delimited fields, got {}: \"{}\"",
            parts.len(),
            line
        ));
    }

    let subject_name = normalize_name(parts[0]);
    let subject_type = parse_node_type(parts[1].trim())?;
    let edge_type = parse_edge_type(parts[2].trim())?;
    let object_name = normalize_name(parts[3]);
    let object_type = parse_node_type(parts[4].trim())?;

    if subject_name.is_empty() || object_name.is_empty() {
        return Err(format!("empty node name in: \"{}\"", line));
    }

    Ok(RawTriple {
        subject_name,
        subject_type,
        edge_type,
        object_name,
        object_type,
    })
}

fn parse_node_type(s: &str) -> Result<NodeType, String> {
    match s.to_lowercase().as_str() {
        "ingredient" => Ok(NodeType::Ingredient),
        "system" => Ok(NodeType::System),
        "mechanism" => Ok(NodeType::Mechanism),
        "property" => Ok(NodeType::Property),
        "symptom" => Ok(NodeType::Symptom),
        "substrate" => Ok(NodeType::Substrate),
        "receptor" => Ok(NodeType::Receptor),
        "pathway" => Ok(NodeType::Pathway),
        "biologicalprocess" | "biological_process" => Ok(NodeType::BiologicalProcess),
        "condition" => Ok(NodeType::Condition),
        "metabolite" => Ok(NodeType::Metabolite),
        "geneprotein" | "gene_protein" | "gene" | "protein" => Ok(NodeType::GeneProtein),
        "celltype" | "cell_type" | "cell" => Ok(NodeType::CellType),
        "microbiota" | "microbiome" => Ok(NodeType::Microbiota),
        other => Err(format!("unknown node type: \"{}\"", other)),
    }
}

fn parse_edge_type(s: &str) -> Result<EdgeType, String> {
    match s.to_lowercase().replace(' ', "_").as_str() {
        "acts_on" => Ok(EdgeType::ActsOn),
        "via_mechanism" => Ok(EdgeType::ViaMechanism),
        "affords" => Ok(EdgeType::Affords),
        "presents_in" => Ok(EdgeType::PresentsIn),
        "contraindicated_with" => Ok(EdgeType::ContraindicatedWith),
        "modulates" => Ok(EdgeType::Modulates),
        "competes_with" => Ok(EdgeType::CompetesWith),
        "disinhibits" => Ok(EdgeType::Disinhibits),
        "sequesters" => Ok(EdgeType::Sequesters),
        "releases" => Ok(EdgeType::Releases),
        "amplifies" => Ok(EdgeType::Amplifies),
        "desensitizes" => Ok(EdgeType::Desensitizes),
        "positively_reinforces" => Ok(EdgeType::PositivelyReinforces),
        "gates" => Ok(EdgeType::Gates),
        other => Err(format!("unknown edge type: \"{}\"", other)),
    }
}

/// Parse the full LLM response into triples + warnings.
///
/// If a lens is provided, triples using node types or edge types outside
/// the lens are rejected with a warning. This is the enforcement layer —
/// even if the LLM ignores the prompt constraints, the parser catches it.
/// Nodes that are too generic to be useful in the graph. LLMs produce these
/// when they're being vague — "body" is not a real system, it's a catch-all
/// that becomes a supernode polluting every traversal path.
const BANNED_SYSTEM_NAMES: &[&str] = &["body", "human body", "the body", "whole body"];

fn is_banned_node(name: &str, node_type: &NodeType) -> bool {
    if *node_type == NodeType::System {
        BANNED_SYSTEM_NAMES.contains(&name.to_lowercase().as_str())
    } else {
        false
    }
}

pub fn parse_triples(raw: &str, lens: Option<&ComplexityLens>) -> (Vec<RawTriple>, Vec<String>) {
    let mut triples = Vec::new();
    let mut warnings = Vec::new();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Skip lines that look like headers or explanations
        if !line.contains('|') {
            continue;
        }
        match parse_line(line) {
            Ok(triple) => {
                // Lens enforcement: reject types outside current complexity
                if let Some(lens) = lens {
                    if !lens.can_see_node(&triple.subject_type) {
                        warnings.push(format!(
                            "node type {:?} exceeds complexity {:.2}, skipping: \"{}\"",
                            triple.subject_type,
                            lens.level(),
                            line
                        ));
                        continue;
                    }
                    if !lens.can_see_node(&triple.object_type) {
                        warnings.push(format!(
                            "node type {:?} exceeds complexity {:.2}, skipping: \"{}\"",
                            triple.object_type,
                            lens.level(),
                            line
                        ));
                        continue;
                    }
                    if !lens.can_see_edge(&triple.edge_type) {
                        warnings.push(format!(
                            "edge type {:?} exceeds complexity {:.2}, skipping: \"{}\"",
                            triple.edge_type,
                            lens.level(),
                            line
                        ));
                        continue;
                    }
                }

                // Type-pair validation: reject invalid source→target combos
                if !triple.edge_type.is_valid_pair(&triple.subject_type, &triple.object_type) {
                    warnings.push(format!(
                        "invalid type pair {:?}→{:?} for edge {:?}, skipping: \"{}\"",
                        triple.subject_type, triple.object_type, triple.edge_type, line
                    ));
                    continue;
                }

                // Supernode filter: reject overly generic nodes that pollute traversal
                if is_banned_node(&triple.subject_name, &triple.subject_type)
                    || is_banned_node(&triple.object_name, &triple.object_type)
                {
                    warnings.push(format!(
                        "banned generic node detected, skipping: \"{}\"",
                        line
                    ));
                    continue;
                }

                // Deduplicate within this batch
                if !triples.contains(&triple) {
                    triples.push(triple);
                }
            }
            Err(w) => warnings.push(w),
        }
    }

    // Cap at 5 triples per extraction
    triples.truncate(5);

    (triples, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_name() {
        assert_eq!(normalize_name("  Muscular_System  "), "muscular system");
        assert_eq!(normalize_name("energy_production"), "energy production");
        assert_eq!(normalize_name("  sleep   quality "), "sleep quality");
        assert_eq!(normalize_name("magnesium"), "magnesium");
    }

    #[test]
    fn test_underscored_names_deduplicate() {
        let raw = "\
magnesium|Ingredient|acts_on|muscular system|System
magnesium|Ingredient|acts_on|muscular_system|System";

        let (triples, _) = parse_triples(raw, None);
        assert_eq!(triples.len(), 1, "underscore variant should dedup with space variant");
    }

    #[test]
    fn test_parse_single_triple() {
        let line = "magnesium|Ingredient|acts_on|nervous system|System";
        let triple = parse_line(line).unwrap();
        assert_eq!(triple.subject_name, "magnesium");
        assert_eq!(triple.subject_type, NodeType::Ingredient);
        assert_eq!(triple.edge_type, EdgeType::ActsOn);
        assert_eq!(triple.object_name, "nervous system");
        assert_eq!(triple.object_type, NodeType::System);
    }

    #[test]
    fn test_parse_triples_multi_line() {
        let raw = "\
magnesium|Ingredient|affords|muscle relaxation|Property
magnesium|Ingredient|acts_on|muscular system|System
magnesium|Ingredient|affords|sleep quality|Property";

        let (triples, warnings) = parse_triples(raw, None);
        assert_eq!(triples.len(), 3);
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_parse_triples_skips_junk() {
        let raw = "\
Here are the triples:
magnesium|Ingredient|affords|muscle relaxation|Property
this is not a triple
magnesium|Ingredient|acts_on|muscular system|System";

        let (triples, warnings) = parse_triples(raw, None);
        assert_eq!(triples.len(), 2);
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_parse_triples_deduplicates() {
        let raw = "\
magnesium|Ingredient|affords|muscle relaxation|Property
magnesium|Ingredient|affords|muscle relaxation|Property";

        let (triples, _) = parse_triples(raw, None);
        assert_eq!(triples.len(), 1);
    }

    #[test]
    fn test_parse_triples_max_five() {
        let raw = "\
magnesium|Ingredient|affords|a|Property
magnesium|Ingredient|affords|b|Property
magnesium|Ingredient|affords|c|Property
magnesium|Ingredient|affords|d|Property
magnesium|Ingredient|affords|e|Property
magnesium|Ingredient|affords|f|Property";

        let (triples, _) = parse_triples(raw, None);
        assert_eq!(triples.len(), 5);
    }

    #[test]
    fn test_bad_node_type_is_warning() {
        let raw = "magnesium|Ingredient|acts_on|nervous system|Organ";
        let (triples, warnings) = parse_triples(raw, None);
        assert!(triples.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("unknown node type"));
    }

    #[test]
    fn test_bad_edge_type_is_warning() {
        let raw = "magnesium|Ingredient|flows_to|nervous system|System";
        let (triples, warnings) = parse_triples(raw, None);
        assert!(triples.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("unknown edge type"));
    }

    #[test]
    fn test_extraction_prompt_includes_nutraceutical() {
        let p = extraction_prompt("Magnesium", "It helps muscles relax.");
        assert!(p.contains("Magnesium"));
        assert!(p.contains("It helps muscles relax."));
    }

    // ── Lens enforcement tests ──────────────────────────────────────────

    #[test]
    fn test_lens_rejects_advanced_edge_at_fifth_grade() {
        let raw = "magnesium|Ingredient|competes_with|calcium|Substrate";
        let lens = ComplexityLens::fifth_grade();
        let (triples, warnings) = parse_triples(raw, Some(&lens));

        assert!(triples.is_empty(), "should reject competes_with at 5th grade");
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("exceeds complexity"));
    }

    #[test]
    fn test_lens_rejects_advanced_node_at_fifth_grade() {
        let raw = "magnesium|Ingredient|acts_on|nmda receptor|Receptor";
        let lens = ComplexityLens::fifth_grade();
        let (triples, warnings) = parse_triples(raw, Some(&lens));

        assert!(triples.is_empty(), "should reject Receptor at 5th grade");
        assert!(!warnings.is_empty());
    }

    #[test]
    fn test_lens_allows_basic_at_fifth_grade() {
        let raw = "magnesium|Ingredient|affords|muscle relaxation|Property";
        let lens = ComplexityLens::fifth_grade();
        let (triples, warnings) = parse_triples(raw, Some(&lens));

        assert_eq!(triples.len(), 1);
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_lens_allows_intermediate_at_tenth_grade() {
        let raw = "magnesium|Ingredient|competes_with|calcium|Substrate";
        let lens = ComplexityLens::tenth_grade();
        let (triples, warnings) = parse_triples(raw, Some(&lens));

        assert_eq!(triples.len(), 1);
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_no_lens_allows_everything() {
        let raw = "magnesium|Ingredient|gates|action potential|Mechanism";
        let (triples, warnings) = parse_triples(raw, None);

        assert_eq!(triples.len(), 1);
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_new_edge_types_parse() {
        let cases = vec![
            ("competes_with", EdgeType::CompetesWith),
            ("disinhibits", EdgeType::Disinhibits),
            ("sequesters", EdgeType::Sequesters),
            ("releases", EdgeType::Releases),
            ("amplifies", EdgeType::Amplifies),
            ("desensitizes", EdgeType::Desensitizes),
            ("positively_reinforces", EdgeType::PositivelyReinforces),
            ("gates", EdgeType::Gates),
        ];
        for (s, expected) in cases {
            assert_eq!(parse_edge_type(s).unwrap(), expected, "failed to parse: {}", s);
        }
    }

    #[test]
    fn test_new_node_types_parse() {
        assert_eq!(parse_node_type("Substrate").unwrap(), NodeType::Substrate);
        assert_eq!(parse_node_type("Receptor").unwrap(), NodeType::Receptor);
    }

    #[test]
    fn test_type_pair_rejects_ingredient_presents_in_system() {
        let raw = "magnesium|Ingredient|presents_in|muscular system|System";
        let (triples, warnings) = parse_triples(raw, None);

        assert!(triples.is_empty(), "Ingredient→presents_in→System should be rejected");
        assert!(warnings.iter().any(|w| w.contains("invalid type pair")));
    }

    #[test]
    fn test_type_pair_allows_symptom_presents_in_system() {
        let raw = "muscle cramps|Symptom|presents_in|muscular system|System";
        let (triples, warnings) = parse_triples(raw, None);

        assert_eq!(triples.len(), 1);
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_type_pair_allows_flexible_affords() {
        // Denylist approach: affords is not restricted, so unusual combos pass
        let raw = "nervous system|System|affords|relaxation|Property";
        let (triples, _) = parse_triples(raw, None);
        assert_eq!(triples.len(), 1);

        let raw2 = "energy metabolism|Mechanism|affords|energy production|Property";
        let (triples2, _) = parse_triples(raw2, None);
        assert_eq!(triples2.len(), 1);
    }

    #[test]
    fn test_type_pair_rejects_ingredient_acts_on_property() {
        let raw = "magnesium|Ingredient|acts_on|relaxation|Property";
        let (triples, warnings) = parse_triples(raw, None);

        assert!(triples.is_empty(), "Ingredient→acts_on→Property should be rejected");
        assert!(warnings.iter().any(|w| w.contains("invalid type pair")));
    }

    #[test]
    fn test_extraction_system_lens_aware() {
        let fifth = ComplexityLens::fifth_grade();
        let prompt = extraction_system(&fifth, &[]);

        assert!(prompt.contains("Ingredient"));
        assert!(prompt.contains("acts_on"));
        assert!(!prompt.contains("Substrate"));
        assert!(!prompt.contains("competes_with"));
        assert!(!prompt.contains("gates"));

        let grad = ComplexityLens::graduate();
        let prompt = extraction_system(&grad, &[]);
        assert!(prompt.contains("Substrate"));
        assert!(prompt.contains("Receptor"));
        assert!(prompt.contains("gates"));
        assert!(prompt.contains("positively_reinforces"));
    }

    #[test]
    fn test_extraction_system_includes_existing_nodes() {
        let lens = ComplexityLens::fifth_grade();
        let prompt = extraction_system(&lens, &["muscle relaxation", "sleep quality", "nervous system"]);

        assert!(prompt.contains("Existing graph nodes"));
        assert!(prompt.contains("muscle relaxation"));
        assert!(prompt.contains("sleep quality"));
        assert!(prompt.contains("nervous system"));
    }

    #[test]
    fn test_extraction_system_omits_section_when_no_nodes() {
        let lens = ComplexityLens::fifth_grade();
        let prompt = extraction_system(&lens, &[]);

        assert!(!prompt.contains("Existing graph nodes"));
    }

    #[test]
    fn test_banned_node_body_filtered() {
        let raw = "magnesium|Ingredient|acts_on|body|System";
        let (triples, warnings) = parse_triples(raw, None);

        assert!(triples.is_empty(), "body as System should be filtered");
        assert!(warnings.iter().any(|w| w.contains("banned generic node")));
    }

    #[test]
    fn test_banned_node_human_body_filtered() {
        let raw = "zinc|Ingredient|acts_on|human body|System";
        let (triples, warnings) = parse_triples(raw, None);

        assert!(triples.is_empty());
        assert!(warnings.iter().any(|w| w.contains("banned generic node")));
    }

    #[test]
    fn test_banned_node_allows_real_systems() {
        let raw = "magnesium|Ingredient|acts_on|muscular system|System";
        let (triples, warnings) = parse_triples(raw, None);

        assert_eq!(triples.len(), 1);
        assert!(warnings.is_empty());
    }
}
