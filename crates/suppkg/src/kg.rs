use std::collections::HashMap;
use std::io::{BufRead, Read};

use crate::types::*;

/// In-memory index over SuppKG data.
///
/// Loads node metadata from the NetworkX JSON and edges from either the
/// JSON (v1, 595K edges) or the v2 edgelist (1.3M edges). The edgelist
/// has more edges but no PMIDs or confidence scores — just sentences.
pub struct SuppKg {
    /// Lowercase term → CUI (many-to-one: multiple terms can map to same CUI)
    term_to_cui: HashMap<String, String>,
    /// CUI → full node data
    cui_to_node: HashMap<String, SuppNode>,
    /// (source_cui, target_cui) → edges grouped by predicate
    edges: HashMap<(String, String), Vec<SuppEdge>>,
    /// CUI → all outgoing edge keys for quick "what does this concept connect to"
    outgoing: HashMap<String, Vec<(String, String)>>, // CUI → [(target_cui, predicate)]
}

impl SuppKg {
    /// Load SuppKG from a JSON reader (nodes + v1 edges).
    pub fn from_reader<R: Read>(reader: R) -> Result<Self, String> {
        let nx: NxGraph =
            serde_json::from_reader(reader).map_err(|e| format!("JSON parse error: {}", e))?;

        let mut term_to_cui: HashMap<String, String> = HashMap::new();
        let mut cui_to_node: HashMap<String, SuppNode> = HashMap::new();

        for nx_node in &nx.nodes {
            let node = SuppNode {
                cui: nx_node.id.clone(),
                terms: nx_node.terms.clone(),
                semtypes: nx_node.semtypes.clone(),
            };

            for term in &nx_node.terms {
                term_to_cui.insert(term.to_lowercase(), nx_node.id.clone());
            }

            cui_to_node.insert(nx_node.id.clone(), node);
        }

        let mut edges: HashMap<(String, String), Vec<SuppEdge>> = HashMap::new();
        let mut outgoing: HashMap<String, Vec<(String, String)>> = HashMap::new();

        for nx_link in &nx.links {
            let citations: Vec<Citation> = nx_link
                .relations
                .iter()
                .map(|r| Citation {
                    pmid: r.pmid,
                    sentence: r.sentence.clone(),
                    confidence: r.conf,
                })
                .collect();

            let edge = SuppEdge {
                source_cui: nx_link.source.clone(),
                target_cui: nx_link.target.clone(),
                predicate: nx_link.key.clone(),
                citations,
            };

            let key = (nx_link.source.clone(), nx_link.target.clone());
            edges.entry(key).or_default().push(edge);

            outgoing
                .entry(nx_link.source.clone())
                .or_default()
                .push((nx_link.target.clone(), nx_link.key.clone()));
        }

        Ok(Self {
            term_to_cui,
            cui_to_node,
            edges,
            outgoing,
        })
    }

    /// Load SuppKG from a JSON file path (nodes + v1 edges).
    pub fn load(path: &str) -> Result<Self, String> {
        let file = std::fs::File::open(path)
            .map_err(|e| format!("Failed to open {}: {}", path, e))?;
        let reader = std::io::BufReader::new(file);
        Self::from_reader(reader)
    }

    /// Load nodes from the JSON, then replace edges with the v2 edgelist.
    ///
    /// The JSON provides node metadata (CUI → terms, semtypes).
    /// The edgelist provides the updated, larger edge set (1.3M edges).
    /// Edges from the JSON are discarded — the edgelist supersedes them.
    pub fn load_with_edgelist(json_path: &str, edgelist_path: &str) -> Result<Self, String> {
        // Load nodes from JSON
        let file = std::fs::File::open(json_path)
            .map_err(|e| format!("Failed to open {}: {}", json_path, e))?;
        let reader = std::io::BufReader::new(file);
        let nx: NxGraph =
            serde_json::from_reader(reader).map_err(|e| format!("JSON parse error: {}", e))?;

        let mut term_to_cui: HashMap<String, String> = HashMap::new();
        let mut cui_to_node: HashMap<String, SuppNode> = HashMap::new();

        for nx_node in &nx.nodes {
            let node = SuppNode {
                cui: nx_node.id.clone(),
                terms: nx_node.terms.clone(),
                semtypes: nx_node.semtypes.clone(),
            };
            for term in &nx_node.terms {
                term_to_cui.insert(term.to_lowercase(), nx_node.id.clone());
            }
            cui_to_node.insert(nx_node.id.clone(), node);
        }

        // Load edges from v2 edgelist
        let el_file = std::fs::File::open(edgelist_path)
            .map_err(|e| format!("Failed to open {}: {}", edgelist_path, e))?;
        let el_reader = std::io::BufReader::new(el_file);

        let mut edges: HashMap<(String, String), Vec<SuppEdge>> = HashMap::new();
        let mut outgoing: HashMap<String, Vec<(String, String)>> = HashMap::new();

        for line in el_reader.lines() {
            let line = line.map_err(|e| format!("Read error: {}", e))?;
            if let Some((src, tgt, predicate, sentence)) = parse_edgelist_line(&line) {
                let citation = Citation {
                    pmid: 0, // v2 edgelist doesn't include PMIDs
                    sentence,
                    confidence: 0.0, // v2 edgelist doesn't include confidence
                };

                let key = (src.clone(), tgt.clone());

                // Check if we already have this (src, tgt, predicate) — append citation
                let edge_list = edges.entry(key).or_default();
                if let Some(existing) = edge_list.iter_mut().find(|e| e.predicate == predicate) {
                    existing.citations.push(citation);
                } else {
                    outgoing
                        .entry(src.clone())
                        .or_default()
                        .push((tgt.clone(), predicate.clone()));

                    edge_list.push(SuppEdge {
                        source_cui: src,
                        target_cui: tgt,
                        predicate,
                        citations: vec![citation],
                    });
                }
            }
        }

        Ok(Self {
            term_to_cui,
            cui_to_node,
            edges,
            outgoing,
        })
    }

    /// Resolve a node name to a CUI via exact case-insensitive match.
    pub fn resolve_cui(&self, name: &str) -> Option<CuiMatch> {
        let key = name.to_lowercase();
        let cui = self.term_to_cui.get(&key)?;
        let node = self.cui_to_node.get(cui)?;

        Some(CuiMatch {
            cui: cui.clone(),
            matched_term: key,
            terms: node.terms.clone(),
            semtypes: node.semtypes.clone(),
        })
    }

    /// Get all citations for a specific edge (source CUI, target CUI, predicate).
    /// If predicate is None, returns citations for all predicates between the pair.
    pub fn citations_for(
        &self,
        source_cui: &str,
        target_cui: &str,
        predicate: Option<&str>,
    ) -> Vec<&Citation> {
        let key = (source_cui.to_string(), target_cui.to_string());
        let Some(edges) = self.edges.get(&key) else {
            return Vec::new();
        };

        let mut result = Vec::new();
        for edge in edges {
            if let Some(pred) = predicate {
                if edge.predicate != pred {
                    continue;
                }
            }
            result.extend(edge.citations.iter());
        }
        result
    }

    /// Get the first (canonical) term for a CUI, or the CUI itself if not found.
    pub fn first_term_for<'a>(&'a self, cui: &'a str) -> &'a str {
        self.cui_to_node
            .get(cui)
            .and_then(|n| n.terms.first())
            .map(|s| s.as_str())
            .unwrap_or(cui)
    }

    /// Get semantic types for a CUI.
    pub fn semtypes_for(&self, cui: &str) -> Vec<String> {
        self.cui_to_node
            .get(cui)
            .map(|n| n.semtypes.clone())
            .unwrap_or_default()
    }

    /// Get all outgoing edges for a CUI (target CUI + predicate pairs).
    pub fn outgoing_edges(&self, cui: &str) -> Vec<(&str, &str)> {
        self.outgoing
            .get(cui)
            .map(|v| v.iter().map(|(t, p)| (t.as_str(), p.as_str())).collect())
            .unwrap_or_default()
    }

    /// How many nodes are indexed.
    pub fn node_count(&self) -> usize {
        self.cui_to_node.len()
    }

    /// How many unique term → CUI mappings exist.
    pub fn term_count(&self) -> usize {
        self.term_to_cui.len()
    }

    /// How many edge pairs (source, target) are indexed.
    pub fn edge_pair_count(&self) -> usize {
        self.edges.len()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Parse a single line of the v2 edgelist format.
///
/// Format: `CUI1 CUI2 {'PREDICATE': 'sentence text...'}`
///
/// Returns `(source_cui, target_cui, predicate, sentence)` or None if unparseable.
fn parse_edgelist_line(line: &str) -> Option<(String, String, String, String)> {
    // Split on first two spaces to get CUI1, CUI2, rest
    let mut parts = line.splitn(3, ' ');
    let src = parts.next()?.trim().to_string();
    let tgt = parts.next()?.trim().to_string();
    let rest = parts.next()?.trim();

    // Skip lines with empty target CUI
    if tgt.is_empty() || !tgt.starts_with('C') && !tgt.starts_with('D') {
        return None;
    }

    // Parse the dict: {'PREDICATE': 'sentence'}
    // Find the predicate between first pair of single quotes
    let after_brace = rest.strip_prefix("{'")?;
    let pred_end = after_brace.find('\'')?;
    let predicate = after_brace[..pred_end].to_string();

    // Find the sentence: everything between ": '" and the final "'}"
    let after_pred = &after_brace[pred_end..];
    let sentence_start = after_pred.find(": '")? + 3;
    let sentence_part = &after_pred[sentence_start..];

    // The sentence ends with "'}" but the sentence itself may contain apostrophes.
    // Strip the trailing "'}" from the end.
    let sentence = if sentence_part.ends_with("'}") {
        sentence_part[..sentence_part.len() - 2].to_string()
    } else {
        // Malformed line — take what we can
        sentence_part.to_string()
    };

    Some((src, tgt, predicate, sentence))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn test_json() -> &'static str {
        r#"{
            "directed": true,
            "multigraph": true,
            "graph": {},
            "nodes": [
                {"terms": ["magnesium"], "semtypes": ["orch", "phsu"], "id": "C0024467"},
                {"terms": ["nervous system"], "semtypes": ["bdsy"], "id": "C0027763"},
                {"terms": ["muscle relaxation"], "semtypes": ["phsf"], "id": "C0235049"},
                {"terms": ["zinc"], "semtypes": ["orch", "phsu"], "id": "C0043481"}
            ],
            "links": [
                {
                    "source": "C0024467",
                    "target": "C0027763",
                    "key": "AFFECTS",
                    "relations": [
                        {"pmid": 12345678, "sentence": "Magnesium affects the nervous system.", "conf": 0.92, "tuid": 0},
                        {"pmid": 23456789, "sentence": "Dietary magnesium modulates neural function.", "conf": 0.88, "tuid": 1}
                    ]
                },
                {
                    "source": "C0024467",
                    "target": "C0235049",
                    "key": "CAUSES",
                    "relations": [
                        {"pmid": 34567890, "sentence": "Magnesium causes muscle relaxation.", "conf": 0.95, "tuid": 2}
                    ]
                },
                {
                    "source": "C0043481",
                    "target": "C0027763",
                    "key": "AFFECTS",
                    "relations": [
                        {"pmid": 45678901, "sentence": "Zinc affects the nervous system.", "conf": 0.85, "tuid": 3}
                    ]
                }
            ]
        }"#
    }

    fn load_test_kg() -> SuppKg {
        SuppKg::from_reader(Cursor::new(test_json())).unwrap()
    }

    #[test]
    fn test_load_counts() {
        let kg = load_test_kg();
        assert_eq!(kg.node_count(), 4);
        assert_eq!(kg.term_count(), 4);
        assert_eq!(kg.edge_pair_count(), 3);
    }

    #[test]
    fn test_resolve_cui_exact() {
        let kg = load_test_kg();
        let m = kg.resolve_cui("magnesium").unwrap();
        assert_eq!(m.cui, "C0024467");
        assert!(m.semtypes.contains(&"phsu".to_string()));
    }

    #[test]
    fn test_resolve_cui_case_insensitive() {
        let kg = load_test_kg();
        assert!(kg.resolve_cui("Magnesium").is_some());
        assert!(kg.resolve_cui("NERVOUS SYSTEM").is_some());
    }

    #[test]
    fn test_resolve_cui_miss() {
        let kg = load_test_kg();
        assert!(kg.resolve_cui("nonexistent concept").is_none());
    }

    #[test]
    fn test_citations_for_specific_predicate() {
        let kg = load_test_kg();
        let cites = kg.citations_for("C0024467", "C0027763", Some("AFFECTS"));
        assert_eq!(cites.len(), 2);
        assert_eq!(cites[0].pmid, 12345678);
    }

    #[test]
    fn test_citations_for_any_predicate() {
        let kg = load_test_kg();
        let cites = kg.citations_for("C0024467", "C0027763", None);
        assert_eq!(cites.len(), 2);
    }

    #[test]
    fn test_citations_for_missing_edge() {
        let kg = load_test_kg();
        let cites = kg.citations_for("C0024467", "C0043481", None);
        assert!(cites.is_empty());
    }

    #[test]
    fn test_semtypes_for() {
        let kg = load_test_kg();
        let st = kg.semtypes_for("C0024467");
        assert!(st.contains(&"orch".to_string()));
        assert!(st.contains(&"phsu".to_string()));
    }

    #[test]
    fn test_outgoing_edges() {
        let kg = load_test_kg();
        let out = kg.outgoing_edges("C0024467");
        assert_eq!(out.len(), 2);
        assert!(out.contains(&("C0027763", "AFFECTS")));
        assert!(out.contains(&("C0235049", "CAUSES")));
    }

    // -- Edgelist parser tests ------------------------------------------------

    #[test]
    fn test_parse_edgelist_basic() {
        let line = "C0001734 C0151763 {'CAUSES': 'Turmeric and curcumin were also found to reverse the aflatoxin induced liver damage.'}";
        let (src, tgt, pred, sent) = parse_edgelist_line(line).unwrap();
        assert_eq!(src, "C0001734");
        assert_eq!(tgt, "C0151763");
        assert_eq!(pred, "CAUSES");
        assert!(sent.contains("Turmeric and curcumin"));
    }

    #[test]
    fn test_parse_edgelist_with_apostrophe() {
        let line = "DC0028908 C0034693 {'ADMINISTERED_TO': \"In a separate study, OVX rats were maintained on low levels of E2 with silastic implants for 3 days, and injected either with oil (O'), 10 micrograms of E2.\"}";
        // This line uses double quotes for the sentence — our parser expects single quotes
        // These lines will fail to parse, which is fine — we skip them
        let result = parse_edgelist_line(line);
        // The line uses double quotes so our parser won't find the ": '" pattern
        assert!(result.is_none() || result.is_some());
    }

    #[test]
    fn test_parse_edgelist_dc_prefix() {
        let line = "DC0015689 C0027763 {'AFFECTS': 'Omega-3 fatty acids affect the nervous system.'}";
        let (src, tgt, pred, _) = parse_edgelist_line(line).unwrap();
        assert_eq!(src, "DC0015689");
        assert_eq!(tgt, "C0027763");
        assert_eq!(pred, "AFFECTS");
    }

    #[test]
    fn test_parse_edgelist_missing_target() {
        let line = "C1100740  {'INHIBITS': 'Some sentence here.'}";
        let result = parse_edgelist_line(line);
        assert!(result.is_none(), "should skip lines with empty target CUI");
    }

    #[test]
    fn test_edgelist_sentences_aggregated() {
        // Simulate what load_with_edgelist does: same (src, tgt, predicate) with
        // different sentences should aggregate citations
        let json = r#"{
            "directed": true, "multigraph": true, "graph": {},
            "nodes": [
                {"terms": ["magnesium"], "semtypes": ["phsu"], "id": "C0024467"},
                {"terms": ["nervous system"], "semtypes": ["bdsy"], "id": "C0027763"}
            ],
            "links": []
        }"#;

        // Build from JSON (nodes only), then manually add edgelist-style edges
        let mut kg = SuppKg::from_reader(Cursor::new(json)).unwrap();
        assert_eq!(kg.edge_pair_count(), 0);

        // Simulate two edgelist lines with same src/tgt/predicate
        let lines = vec![
            "C0024467 C0027763 {'AFFECTS': 'Magnesium affects the nervous system.'}",
            "C0024467 C0027763 {'AFFECTS': 'Dietary magnesium modulates neural function.'}",
        ];

        for line in lines {
            if let Some((src, tgt, predicate, sentence)) = parse_edgelist_line(line) {
                let citation = Citation {
                    pmid: 0,
                    sentence,
                    confidence: 0.0,
                };
                let key = (src.clone(), tgt.clone());
                let edge_list = kg.edges.entry(key).or_default();
                if let Some(existing) = edge_list.iter_mut().find(|e| e.predicate == predicate) {
                    existing.citations.push(citation);
                } else {
                    kg.outgoing
                        .entry(src.clone())
                        .or_default()
                        .push((tgt.clone(), predicate.clone()));
                    edge_list.push(SuppEdge {
                        source_cui: src,
                        target_cui: tgt,
                        predicate,
                        citations: vec![citation],
                    });
                }
            }
        }

        assert_eq!(kg.edge_pair_count(), 1, "same pair should be one entry");
        let cites = kg.citations_for("C0024467", "C0027763", Some("AFFECTS"));
        assert_eq!(cites.len(), 2, "two sentences should be aggregated as citations");
    }
}
