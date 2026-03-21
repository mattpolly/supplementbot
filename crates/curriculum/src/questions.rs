use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Curriculum stages and question templates
// ---------------------------------------------------------------------------

/// A curriculum question ready to send to an LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurriculumQuestion {
    /// The nutraceutical being studied
    pub nutraceutical: String,
    /// Which stage this question belongs to
    pub stage: Stage,
    /// What kind of knowledge this question targets
    pub question_type: QuestionType,
    /// The fully rendered prompt text
    pub prompt: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stage {
    /// Stage 1: basic systems, mechanisms, therapeutic uses
    Foundational,
    /// Stage 2: cross-system links, contraindications, interactions
    Relational,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuestionType {
    /// "What physiological systems does X act on?"
    Systems,
    /// "What are the known mechanisms of action?"
    Mechanisms,
    /// "What are the primary therapeutic uses?"
    TherapeuticUses,
    /// "How does X's effect on [system A] relate to its [system B] properties?"
    CrossSystemRelation,
    /// "What are the contraindications when combined with [Y]?"
    Contraindications,
}

// ---------------------------------------------------------------------------
// System prompt — shared across all curriculum questions
// ---------------------------------------------------------------------------

const SYSTEM_PROMPT: &str = "\
You are a nutraceutical knowledge extraction assistant. Your role is to provide \
structured, evidence-based information about supplements and their physiological effects.

Rules:
- Be specific about mechanisms of action, naming receptors, pathways, and enzymes where known.
- Distinguish between well-established effects and preliminary/emerging research.
- Name the physiological systems affected using standard terminology: \
  Nervous, Gastrointestinal, Musculoskeletal, Immune.
- When describing mechanisms, be precise: name the mechanism (e.g. \"NMDA receptor antagonism\") \
  rather than giving vague descriptions.
- Do not discuss diseases or diagnoses. Frame everything in terms of symptoms and physiological function.
- Structure your response clearly with one point per line where possible.";

pub fn system_prompt() -> &'static str {
    SYSTEM_PROMPT
}

// ---------------------------------------------------------------------------
// Stage 1 — Foundational questions
// ---------------------------------------------------------------------------

pub fn stage1_questions(nutraceutical: &str) -> Vec<CurriculumQuestion> {
    let templates: Vec<(QuestionType, String)> = vec![
        (
            QuestionType::Systems,
            format!(
                "What physiological systems does {} act on? \
                 For each system, briefly describe the primary effect. \
                 Focus on these systems: Nervous, Gastrointestinal, Musculoskeletal, Immune.",
                nutraceutical
            ),
        ),
        (
            QuestionType::Mechanisms,
            format!(
                "What are the known mechanisms of action for {}? \
                 For each mechanism, name the specific receptor, enzyme, pathway, or process involved. \
                 Be precise — e.g. \"NMDA receptor antagonism\" rather than \"affects the brain.\"",
                nutraceutical
            ),
        ),
        (
            QuestionType::TherapeuticUses,
            format!(
                "What are the primary therapeutic uses of {} as a supplement? \
                 For each use, describe which symptoms it addresses and through which mechanism. \
                 Do not mention diseases or diagnoses — frame everything in terms of symptoms and physiological function.",
                nutraceutical
            ),
        ),
    ];

    templates
        .into_iter()
        .map(|(question_type, prompt)| CurriculumQuestion {
            nutraceutical: nutraceutical.to_string(),
            stage: Stage::Foundational,
            question_type,
            prompt,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Stage 2 — Relational questions (placeholder — built out in Phase 3+)
// ---------------------------------------------------------------------------

pub fn stage2_questions(nutraceutical: &str, related_systems: &[&str]) -> Vec<CurriculumQuestion> {
    let mut questions = Vec::new();

    // Cross-system relationship questions — one per pair of systems
    if related_systems.len() >= 2 {
        for i in 0..related_systems.len() {
            for j in (i + 1)..related_systems.len() {
                questions.push(CurriculumQuestion {
                    nutraceutical: nutraceutical.to_string(),
                    stage: Stage::Relational,
                    question_type: QuestionType::CrossSystemRelation,
                    prompt: format!(
                        "How does {}'s effect on the {} system relate to its {} system properties? \
                         Describe any shared mechanisms, feedback loops, or downstream effects that connect these two systems.",
                        nutraceutical, related_systems[i], related_systems[j]
                    ),
                });
            }
        }
    }

    // Contraindication question
    questions.push(CurriculumQuestion {
        nutraceutical: nutraceutical.to_string(),
        stage: Stage::Relational,
        question_type: QuestionType::Contraindications,
        prompt: format!(
            "What are the known contraindications for {}? \
             Include interactions with common medications, other supplements, \
             and physiological conditions where {} should be avoided or used with caution.",
            nutraceutical, nutraceutical
        ),
    });

    questions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stage1_generates_three_questions() {
        let questions = stage1_questions("Magnesium");
        assert_eq!(questions.len(), 3);

        assert_eq!(questions[0].question_type, QuestionType::Systems);
        assert_eq!(questions[1].question_type, QuestionType::Mechanisms);
        assert_eq!(questions[2].question_type, QuestionType::TherapeuticUses);

        for q in &questions {
            assert_eq!(q.nutraceutical, "Magnesium");
            assert_eq!(q.stage, Stage::Foundational);
            assert!(q.prompt.contains("Magnesium"));
        }
    }

    #[test]
    fn test_stage2_generates_cross_system_questions() {
        let questions = stage2_questions("Magnesium", &["Nervous", "Gastrointestinal", "Immune"]);

        // 3 cross-system pairs + 1 contraindication = 4
        assert_eq!(questions.len(), 4);

        let cross_system: Vec<_> = questions
            .iter()
            .filter(|q| q.question_type == QuestionType::CrossSystemRelation)
            .collect();
        assert_eq!(cross_system.len(), 3);

        let contra: Vec<_> = questions
            .iter()
            .filter(|q| q.question_type == QuestionType::Contraindications)
            .collect();
        assert_eq!(contra.len(), 1);
    }

    #[test]
    fn test_system_prompt_contains_legal_constraint() {
        let prompt = system_prompt();
        assert!(prompt.contains("Do not discuss diseases or diagnoses"));
    }
}
