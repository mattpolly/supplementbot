use std::collections::HashSet;
use std::path::Path;

use regex::Regex;

// ---------------------------------------------------------------------------
// Red flag ejector — hard-coded safety check that runs BEFORE any graph
// reasoning or LLM call. Pattern-matches on emergency keywords in user input.
//
// When triggered: immediately break intake flow, show static emergency
// resource block. Session is flagged; no further supplement discussion occurs.
// ---------------------------------------------------------------------------

/// Emergency keywords that indicate the user needs immediate medical help,
/// NOT supplement advice. These are checked case-insensitively.
const RED_FLAGS: &[&str] = &[
    "chest pain",
    "heart attack",
    "stroke",
    "can't breathe",
    "cannot breathe",
    "difficulty breathing",
    "suicidal",
    "want to die",
    "kill myself",
    "end my life",
    "overdose",
    "sudden numbness",
    "vision loss",
    "severe bleeding",
    "allergic reaction",
    "anaphylaxis",
    "seizure",
    "choking",
    "unconscious",
    "passed out",
    "blood in stool",
    "blood in urine",
    "severe headache",
    "worst headache",
];

/// Result of the red flag check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafetyCheck {
    /// No emergency detected — proceed with normal intake.
    Clear,
    /// Emergency detected — stop everything, show emergency resources.
    /// Contains the matched flag phrase for logging.
    EmergencyExit(String),
}

/// Check user input for emergency red flags.
/// This runs BEFORE any graph reasoning or LLM call.
pub fn check_red_flags(input: &str) -> SafetyCheck {
    let lower = input.to_lowercase();
    for &flag in RED_FLAGS {
        if lower.contains(flag) {
            return SafetyCheck::EmergencyExit(flag.to_string());
        }
    }
    SafetyCheck::Clear
}

// ---------------------------------------------------------------------------
// Post-generation safety filter — deterministic scan of LLM output before
// it reaches the user. Prompt-level constraints are necessary but NOT
// sufficient; this catches violations the LLM slips through.
// ---------------------------------------------------------------------------

/// Patterns that indicate the LLM crossed a legal line.
/// Order matters: more severe patterns first.
static BLACKLIST_PATTERNS: &[&str] = &[
    // Diagnosis language
    r"(?i)\byou have\s+(?:a |an )?(?:\w+\s+)?(?:deficiency|condition|disorder|disease|infection|syndrome|illness)\b",
    r"(?i)\byou(?:'re| are) (?:suffering from|diagnosed with)\b",
    r"(?i)\bI (?:can |would )?diagnos[ei]",
    r"(?i)\bdiagnosis\b",
    // Cure/treat language
    r"(?i)\bcures?\b",
    r"(?i)\btreat(?:s|ing|ment of)?\s+(?:your |the )?\b(?:disease|condition|disorder|illness)\b",
    // Dosage prescriptions (we don't do dosage)
    r"(?i)\btake\s+\d+\s*(?:mg|g|ml|mcg|iu)\b",
    r"(?i)\bI (?:can |will |would )?prescribe\b",
];

/// Result of the post-generation filter.
#[derive(Debug, Clone)]
pub enum FilterResult {
    /// Safe to send to user as-is.
    Pass(String),
    /// Violation found — the LLM should be re-prompted with stricter instruction.
    /// Contains the matched pattern description for logging.
    Rewrite { original: String, violation: String },
    /// Severe violation — fall back to a canned safe response.
    Block { violation: String },
}

/// A compiled set of blacklist patterns for efficient repeated checking.
pub struct SafetyFilter {
    patterns: Vec<(Regex, &'static str)>,
    /// Every known supplement name and synonym from the ingredient registry.
    /// Used to detect ingredient-like terms in responses via n-gram scan.
    /// Empty if no ingredient_names.json was loaded (filter degrades gracefully).
    pub known_ingredient_names: HashSet<String>,
}

impl SafetyFilter {
    /// Compile blacklist patterns and optionally load the ingredient registry.
    ///
    /// `ingredient_names_path` should point to `known_supplement_terms.json`
    /// exported by `supplementology export known-supplement-terms` (flat string
    /// array) or `ingredient_names.json` (structured object array). Both formats
    /// are detected automatically. If `None` or the file is not found, the
    /// ingredient whitelist check is skipped and degrades gracefully.
    pub fn new(ingredient_names_path: Option<&Path>) -> Self {
        let patterns = BLACKLIST_PATTERNS
            .iter()
            .map(|p| {
                (
                    Regex::new(p).unwrap_or_else(|e| panic!("bad safety regex '{p}': {e}")),
                    *p,
                )
            })
            .collect();

        let known_ingredient_names = ingredient_names_path
            .and_then(|path| {
                if !path.exists() {
                    eprintln!("[safety] {path:?} not found — ingredient whitelist check disabled");
                    return None;
                }
                let raw = std::fs::read_to_string(path).ok()?;
                let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
                let mut set = HashSet::new();
                if let Some(entries) = parsed.as_array() {
                    for entry in entries {
                        if let Some(s) = entry.as_str() {
                            // Flat string array format (known_supplement_terms.json)
                            set.insert(s.to_lowercase());
                        } else {
                            // Structured object format (ingredient_names.json)
                            if let Some(name) = entry.get("canonical_name").and_then(|v| v.as_str()) {
                                set.insert(name.to_lowercase());
                            }
                            if let Some(names) = entry.get("names").and_then(|v| v.as_array()) {
                                for n in names {
                                    if let Some(s) = n.as_str() {
                                        set.insert(s.to_lowercase());
                                    }
                                }
                            }
                        }
                    }
                }
                eprintln!("[safety] ingredient registry loaded: {} known names", set.len());
                Some(set)
            })
            .unwrap_or_default();

        Self { patterns, known_ingredient_names }
    }

    /// Check LLM output for legal violations.
    ///
    /// `permitted` is the current turn's candidate whitelist (ingredient names
    /// the LLM is explicitly allowed to mention). Any ingredient name found in
    /// the response that is NOT in this list is blocked.
    pub fn check(&self, llm_output: &str, permitted: &[String]) -> FilterResult {
        // --- 1. Existing blacklist pattern checks ---
        for (regex, pattern_src) in &self.patterns {
            if regex.is_match(llm_output) {
                let violation = pattern_src.to_string();
                if pattern_src.contains("diagnos")
                    || pattern_src.contains("cure")
                    || pattern_src.contains("prescri")
                {
                    return FilterResult::Block { violation };
                }
                return FilterResult::Rewrite {
                    original: llm_output.to_string(),
                    violation,
                };
            }
        }

        // --- 2. Ingredient whitelist check ---
        // Only runs if the registry was loaded. Scans the response for n-grams
        // (unigrams, bigrams, trigrams) that match a known ingredient name.
        // Any match that is NOT in the permitted list is a violation.
        if !self.known_ingredient_names.is_empty() {
            let permitted_lower: HashSet<String> =
                permitted.iter().map(|s| s.to_lowercase()).collect();

            // Tokenize response into lowercase words, stripping punctuation
            let words: Vec<&str> = llm_output
                .split_whitespace()
                .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
                .filter(|w| !w.is_empty())
                .collect();

            // Check unigrams, bigrams, and trigrams
            for window_size in 1..=3usize {
                for window in words.windows(window_size) {
                    let ngram = window.join(" ").to_lowercase();
                    if self.known_ingredient_names.contains(&ngram)
                        && !permitted_lower.contains(&ngram)
                    {
                        let violation = format!(
                            "ingredient '{}' mentioned but not in permitted list {:?}",
                            ngram,
                            permitted.iter().collect::<Vec<_>>()
                        );
                        eprintln!("[safety] BLOCK — {violation}");
                        return FilterResult::Block { violation };
                    }
                }
            }
        }

        FilterResult::Pass(llm_output.to_string())
    }
}

impl Default for SafetyFilter {
    fn default() -> Self {
        Self::new(None)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Red flag ejector tests --

    #[test]
    fn test_red_flag_chest_pain() {
        assert!(matches!(
            check_red_flags("I'm having chest pain"),
            SafetyCheck::EmergencyExit(flag) if flag == "chest pain"
        ));
    }

    #[test]
    fn test_red_flag_suicidal() {
        assert!(matches!(
            check_red_flags("I feel suicidal"),
            SafetyCheck::EmergencyExit(flag) if flag == "suicidal"
        ));
    }

    #[test]
    fn test_red_flag_case_insensitive() {
        assert!(matches!(
            check_red_flags("I think I'm having a HEART ATTACK"),
            SafetyCheck::EmergencyExit(_)
        ));
    }

    #[test]
    fn test_no_red_flag_normal_input() {
        assert_eq!(
            check_red_flags("my legs hurt at night and I can't sleep"),
            SafetyCheck::Clear
        );
    }

    #[test]
    fn test_no_red_flag_empty_input() {
        assert_eq!(check_red_flags(""), SafetyCheck::Clear);
    }

    // -- Post-generation filter tests --

    #[test]
    fn test_filter_passes_safe_output() {
        let filter = SafetyFilter::new(None);
        let output = "Magnesium acts on the muscular system where your symptoms present. \
                       It may help support muscle relaxation.";
        assert!(matches!(filter.check(output, &["Magnesium".to_string()]), FilterResult::Pass(_)));
    }

    #[test]
    fn test_filter_blocks_diagnosis() {
        let filter = SafetyFilter::new(None);
        let output = "Based on your symptoms, I can diagnose you with magnesium deficiency.";
        assert!(matches!(filter.check(output, &[]), FilterResult::Block { .. }));
    }

    #[test]
    fn test_filter_blocks_cure() {
        let filter = SafetyFilter::new(None);
        let output = "Magnesium cures muscle cramps effectively.";
        assert!(matches!(filter.check(output, &[]), FilterResult::Block { .. }));
    }

    #[test]
    fn test_filter_rewrites_you_have() {
        let filter = SafetyFilter::new(None);
        let output = "It sounds like you have a magnesium deficiency.";
        assert!(matches!(filter.check(output, &[]), FilterResult::Rewrite { .. }));
    }

    #[test]
    fn test_filter_blocks_dosage() {
        let filter = SafetyFilter::new(None);
        let output = "I'd recommend you take 400mg of magnesium daily.";
        // "prescri" won't match here, but "take \d+ mg" will — that's a Rewrite
        assert!(!matches!(filter.check(output, &[]), FilterResult::Pass(_)));
    }

    #[test]
    fn test_filter_blocks_unlisted_ingredient() {
        use std::collections::HashSet;
        // Simulate a registry with stinging nettle known
        let mut filter = SafetyFilter::new(None);
        filter.known_ingredient_names = HashSet::from([
            "stinging nettle".to_string(),
            "quercetin".to_string(),
        ]);
        let output = "Stinging nettle extract is a great option for allergy relief.";
        let permitted = vec!["Quercetin".to_string()];
        assert!(matches!(filter.check(output, &permitted), FilterResult::Block { .. }));
    }

    #[test]
    fn test_filter_passes_permitted_ingredient() {
        use std::collections::HashSet;
        let mut filter = SafetyFilter::new(None);
        filter.known_ingredient_names = HashSet::from([
            "quercetin".to_string(),
        ]);
        let output = "Quercetin may help support a balanced immune response.";
        let permitted = vec!["Quercetin".to_string()];
        assert!(matches!(filter.check(output, &permitted), FilterResult::Pass(_)));
    }
}
