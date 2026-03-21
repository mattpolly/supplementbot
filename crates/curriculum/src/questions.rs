use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Curriculum — progressive complexity through grade levels
//
// Each level re-explains the same nutraceutical at increasing depth.
// The graph grows deeper, not wider. Earlier levels establish coarse nodes;
// later levels add precision and named mechanisms.
//
// Level 1 (5th grade):   Simple effects. What does it do in plain language?
// Level 2 (10th grade):   Basic mechanisms. How does it work?
// Level 3 (College Sophomore):  Specific biochemistry, interactions, nuance.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GradeLevel {
    /// 5th grade — simple, concrete effects
    Fifth,
    /// 10th grade — basic mechanisms
    Tenth,
    /// College sophomore nutrition course — biochemistry
    College,
}

impl GradeLevel {
    pub fn label(&self) -> &'static str {
        match self {
            GradeLevel::Fifth => "5th Grade",
            GradeLevel::Tenth => "10th Grade",
            GradeLevel::College => "College Sophomore",
        }
    }

    /// All levels in order
    pub fn all() -> &'static [GradeLevel] {
        &[
            GradeLevel::Fifth,
            GradeLevel::Tenth,
            GradeLevel::College,
        ]
    }

    fn audience_description(&self) -> &'static str {
        match self {
            GradeLevel::Fifth => "a 5th grader (10 years old). Use simple everyday words. \
                No scientific terms. Focus on what it does to the body that a kid could understand",
            GradeLevel::Tenth => "a 10th grader (15 years old) in a basic biology class. \
                You can use simple scientific terms but explain them. \
                Focus on how it works at a basic level",
            GradeLevel::College => "a college sophomore in a nutrition science course. \
                Use biochemical details — enzyme names, receptor subtypes, \
                signaling cascades, and interaction mechanisms. Be thorough but concise",
        }
    }
}

/// A single curriculum question
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurriculumQuestion {
    /// The nutraceutical being studied
    pub nutraceutical: String,
    /// The complexity level
    pub grade_level: GradeLevel,
    /// The concept being explored (None for the top-level question)
    pub concept: Option<String>,
    /// The fully rendered prompt text
    pub prompt: String,
}

// ---------------------------------------------------------------------------
// System prompt — constrains responses and enforces legal boundary
// ---------------------------------------------------------------------------

const SYSTEM_PROMPT: &str = "\
You are a nutraceutical knowledge extraction assistant.

Rules:
- Answer in exactly ONE sentence. Be concise but specific.
- Do not discuss diseases or diagnoses. Frame everything in terms of \
  symptoms and physiological function.
- Do not speculate beyond well-established knowledge.
- Match your vocabulary to the audience level specified in the question.
- Do not hedge with unnecessary qualifiers. Be direct.";

pub fn system_prompt() -> &'static str {
    SYSTEM_PROMPT
}

// ---------------------------------------------------------------------------
// Concept extraction prompt
// ---------------------------------------------------------------------------

const EXTRACT_PROMPT: &str = "\
You are a concept extraction assistant. Given a sentence about a supplement, \
extract the key physiological concepts that could be explored further.

Rules:
- Return ONLY a comma-separated list of short noun phrases (2-4 words each).
- Return at most 5 concepts.
- Focus on concrete physiological terms: body systems, mechanisms, processes, substances.
- Do not include the supplement name itself.
- Do not include vague words like \"health\", \"function\", \"body\", or \"important\".
- Do not include filler words or explanations. Just the list.

Example input: \"Magnesium helps your muscles relax and helps you sleep better.\"
Example output: muscle relaxation, sleep quality";

pub fn extract_prompt() -> &'static str {
    EXTRACT_PROMPT
}

// ---------------------------------------------------------------------------
// Question generators
// ---------------------------------------------------------------------------

/// The top-level question for a given grade level.
pub fn level_question(nutraceutical: &str, level: GradeLevel) -> CurriculumQuestion {
    let audience = level.audience_description();
    CurriculumQuestion {
        nutraceutical: nutraceutical.to_string(),
        grade_level: level,
        concept: None,
        prompt: format!(
            "Explain to {} what {} does as a supplement, in one sentence.",
            audience, nutraceutical
        ),
    }
}

/// A drill-down question: re-explain a specific concept at the given grade level.
pub fn concept_question(
    nutraceutical: &str,
    concept: &str,
    level: GradeLevel,
) -> CurriculumQuestion {
    let audience = level.audience_description();
    CurriculumQuestion {
        nutraceutical: nutraceutical.to_string(),
        grade_level: level,
        concept: Some(concept.to_string()),
        prompt: format!(
            "Explain to {} what {} has to do with {}, in one sentence.",
            audience, nutraceutical, concept
        ),
    }
}

/// The concept extraction question — sent after each answer.
pub fn extraction_question(sentence: &str) -> String {
    format!(
        "Extract the key physiological concepts from this sentence:\n\"{}\"",
        sentence
    )
}

/// Parse the comma-separated concept list from the LLM's extraction response.
pub fn parse_concepts(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .filter(|s| s.split_whitespace().count() <= 5)
        .take(5)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_level_question_contains_audience() {
        let q = level_question("Magnesium", GradeLevel::Fifth);
        assert!(q.prompt.contains("5th grader"));
        assert!(q.prompt.contains("Magnesium"));
        assert!(q.prompt.contains("one sentence"));
    }

    #[test]
    fn test_concept_question() {
        let q = concept_question("Magnesium", "muscle relaxation", GradeLevel::Tenth);
        assert!(q.prompt.contains("10th grader"));
        assert!(q.prompt.contains("muscle relaxation"));
        assert_eq!(q.concept.as_deref(), Some("muscle relaxation"));
    }

    #[test]
    fn test_all_levels() {
        let levels = GradeLevel::all();
        assert_eq!(levels.len(), 3);
        assert_eq!(levels[0], GradeLevel::Fifth);
        assert_eq!(levels[2], GradeLevel::College);
    }

    #[test]
    fn test_parse_concepts() {
        let raw = "muscle relaxation, calcium channels, neuromuscular junction";
        let concepts = parse_concepts(raw);
        assert_eq!(concepts.len(), 3);
        assert_eq!(concepts[0], "muscle relaxation");
    }

    #[test]
    fn test_parse_concepts_max_five() {
        let raw = "a, b, c, d, e, f, g";
        let concepts = parse_concepts(raw);
        assert_eq!(concepts.len(), 5);
    }

    #[test]
    fn test_system_prompt_legal_constraint() {
        assert!(system_prompt().contains("Do not discuss diseases or diagnoses"));
    }

    #[test]
    fn test_system_prompt_one_sentence() {
        assert!(system_prompt().contains("ONE sentence"));
    }
}
