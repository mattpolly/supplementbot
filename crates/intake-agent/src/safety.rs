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
    r"(?i)\byou have\b",
    r"(?i)\byou(?:'re| are) (?:suffering from|diagnosed with)\b",
    r"(?i)\bI (?:can |would )?diagnos[ei]",
    r"(?i)\bdiagnosis\b",
    // Cure/treat language
    r"(?i)\bcures?\b",
    r"(?i)\btreat(?:s|ing|ment of)?\s+(?:your |the )?\b(?:disease|condition|disorder|illness)\b",
    // Dosage prescriptions (we don't do dosage)
    r"(?i)\btake\s+\d+\s*(?:mg|g|ml|mcg|iu)\b",
    r"(?i)\bprescri(?:be|ption)\b",
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
}

impl SafetyFilter {
    /// Compile blacklist patterns. Call once at startup.
    pub fn new() -> Self {
        let patterns = BLACKLIST_PATTERNS
            .iter()
            .map(|p| {
                (
                    Regex::new(p).unwrap_or_else(|e| panic!("bad safety regex '{p}': {e}")),
                    *p,
                )
            })
            .collect();
        Self { patterns }
    }

    /// Check LLM output for legal violations.
    pub fn check(&self, llm_output: &str) -> FilterResult {
        for (regex, pattern_src) in &self.patterns {
            if regex.is_match(llm_output) {
                let violation = pattern_src.to_string();
                // Diagnosis and cure patterns are severe — block entirely
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
        FilterResult::Pass(llm_output.to_string())
    }
}

impl Default for SafetyFilter {
    fn default() -> Self {
        Self::new()
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
        let filter = SafetyFilter::new();
        let output = "Magnesium acts on the muscular system where your symptoms present. \
                       It may help support muscle relaxation.";
        assert!(matches!(filter.check(output), FilterResult::Pass(_)));
    }

    #[test]
    fn test_filter_blocks_diagnosis() {
        let filter = SafetyFilter::new();
        let output = "Based on your symptoms, I can diagnose you with magnesium deficiency.";
        assert!(matches!(filter.check(output), FilterResult::Block { .. }));
    }

    #[test]
    fn test_filter_blocks_cure() {
        let filter = SafetyFilter::new();
        let output = "Magnesium cures muscle cramps effectively.";
        assert!(matches!(filter.check(output), FilterResult::Block { .. }));
    }

    #[test]
    fn test_filter_rewrites_you_have() {
        let filter = SafetyFilter::new();
        let output = "It sounds like you have a magnesium deficiency.";
        assert!(matches!(filter.check(output), FilterResult::Rewrite { .. }));
    }

    #[test]
    fn test_filter_blocks_dosage() {
        let filter = SafetyFilter::new();
        let output = "I'd recommend you take 400mg of magnesium daily.";
        // "prescri" won't match here, but "take \d+ mg" will — that's a Rewrite
        assert!(!matches!(filter.check(output), FilterResult::Pass(_)));
    }
}
