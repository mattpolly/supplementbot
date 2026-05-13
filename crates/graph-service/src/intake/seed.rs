// ---------------------------------------------------------------------------
// Seed data for the intake knowledge graph.
//
// Populates the core structure: stages, archetypes, goals, questions,
// exit conditions, system reviews, graph actions, and all edges.
// ---------------------------------------------------------------------------

use super::pg_store::PgIntakeStore;
use super::types::*;

/// Build a PgIntakeStore from the static seed data (no network, no DB).
/// Used in tests that need an intake store without a running API.
pub fn build_seed_store() -> PgIntakeStore {
    use OldcartsDimension::*;

    let archetypes = vec![
        ArchetypeProfile { id: "pain".into(), name: "Pain".into(), sufficient_dimensions: 4,
            relevant_oldcarts: vec![Onset, Location, Character, Severity, Aggravating, Timing],
            irrelevant_oldcarts: vec![],
            default_systems: vec!["musculoskeletal system".into(), "nervous system".into()] },
        ArchetypeProfile { id: "sleep".into(), name: "Sleep".into(), sufficient_dimensions: 3,
            relevant_oldcarts: vec![Onset, Duration, Timing, Aggravating, Alleviating],
            irrelevant_oldcarts: vec![Location, Radiation],
            default_systems: vec!["nervous system".into(), "endocrine system".into()] },
        ArchetypeProfile { id: "mood".into(), name: "Mood".into(), sufficient_dimensions: 3,
            relevant_oldcarts: vec![Onset, Duration, Character, Severity, Timing],
            irrelevant_oldcarts: vec![Location, Radiation],
            default_systems: vec!["nervous system".into(), "endocrine system".into()] },
        ArchetypeProfile { id: "digestive".into(), name: "Digestive".into(), sufficient_dimensions: 4,
            relevant_oldcarts: vec![Onset, Location, Character, Timing, Aggravating],
            irrelevant_oldcarts: vec![Radiation],
            default_systems: vec!["digestive system".into(), "immune system".into()] },
        ArchetypeProfile { id: "fatigue".into(), name: "Fatigue / Energy".into(), sufficient_dimensions: 3,
            relevant_oldcarts: vec![Onset, Duration, Severity, Timing, Aggravating],
            irrelevant_oldcarts: vec![Location, Radiation, Character],
            default_systems: vec!["endocrine system".into(), "immune system".into(), "nervous system".into()] },
        ArchetypeProfile { id: "skin".into(), name: "Skin / Integumentary".into(), sufficient_dimensions: 3,
            relevant_oldcarts: vec![Onset, Location, Character, Aggravating],
            irrelevant_oldcarts: vec![Radiation, Timing],
            default_systems: vec!["immune system".into(), "integumentary system".into()] },
        ArchetypeProfile { id: "respiratory".into(), name: "Respiratory".into(), sufficient_dimensions: 3,
            relevant_oldcarts: vec![Onset, Character, Timing, Aggravating, Severity],
            irrelevant_oldcarts: vec![Location, Radiation],
            default_systems: vec!["respiratory system".into(), "immune system".into()] },
        ArchetypeProfile { id: "cardiovascular".into(), name: "Cardiovascular".into(), sufficient_dimensions: 4,
            relevant_oldcarts: vec![Onset, Character, Timing, Severity, Radiation],
            irrelevant_oldcarts: vec![],
            default_systems: vec!["cardiovascular system".into(), "nervous system".into()] },
        ArchetypeProfile { id: "immune".into(), name: "Immune / Inflammatory".into(), sufficient_dimensions: 3,
            relevant_oldcarts: vec![Onset, Duration, Timing, Severity],
            irrelevant_oldcarts: vec![Radiation, Character],
            default_systems: vec!["immune system".into()] },
        ArchetypeProfile { id: "cognitive".into(), name: "Cognitive / Neurological".into(), sufficient_dimensions: 3,
            relevant_oldcarts: vec![Onset, Duration, Character, Timing, Severity],
            irrelevant_oldcarts: vec![Radiation],
            default_systems: vec!["nervous system".into()] },
    ];

    let profiles = vec![
        SymptomProfile { id: "headache".into(), name: "Headache".into(), cui: Some("C0018681".into()),
            aliases: vec!["headaches".into(), "migraine".into()], archetype_id: "cognitive".into(),
            sufficient_dimensions_override: Some(3), relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None, associated_systems: vec!["nervous system".into()] },
        SymptomProfile { id: "muscle_cramps".into(), name: "Muscle Cramps".into(), cui: Some("C0026821".into()),
            aliases: vec!["cramps".into(), "leg cramps".into()], archetype_id: "pain".into(),
            sufficient_dimensions_override: Some(3), relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            associated_systems: vec!["musculoskeletal system".into(), "nervous system".into()] },
        SymptomProfile { id: "insomnia".into(), name: "Insomnia".into(), cui: Some("C0917801".into()),
            aliases: vec!["trouble sleeping".into(), "can't sleep".into()], archetype_id: "sleep".into(),
            sufficient_dimensions_override: Some(3), relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            associated_systems: vec!["nervous system".into(), "endocrine system".into()] },
        SymptomProfile { id: "fatigue".into(), name: "Fatigue".into(), cui: Some("C0015672".into()),
            aliases: vec!["tired".into(), "low energy".into()], archetype_id: "fatigue".into(),
            sufficient_dimensions_override: Some(3), relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            associated_systems: vec!["endocrine system".into(), "nervous system".into()] },
        SymptomProfile { id: "anxiety".into(), name: "Anxiety".into(), cui: Some("C0003469".into()),
            aliases: vec!["anxious".into(), "stress".into()], archetype_id: "mood".into(),
            sufficient_dimensions_override: Some(3), relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            associated_systems: vec!["nervous system".into(), "endocrine system".into()] },
    ];

    let questions = vec![
        QuestionTemplate { id: "what_brings_you_in".into(), template: "What brings you in today?".into(),
            oldcarts_dimension: None, system_name: None },
        QuestionTemplate { id: "anything_else".into(), template: "Is there anything else bothering you?".into(),
            oldcarts_dimension: None, system_name: None },
        QuestionTemplate { id: "ask_onset".into(), template: "When did {symptom} start?".into(),
            oldcarts_dimension: Some(Onset), system_name: None },
        QuestionTemplate { id: "ask_location".into(), template: "Where exactly do you feel {symptom}?".into(),
            oldcarts_dimension: Some(Location), system_name: None },
        QuestionTemplate { id: "ask_duration".into(), template: "How long does {symptom} typically last?".into(),
            oldcarts_dimension: Some(Duration), system_name: None },
        QuestionTemplate { id: "ask_character".into(), template: "What does {symptom} feel like?".into(),
            oldcarts_dimension: Some(Character), system_name: None },
        QuestionTemplate { id: "ask_aggravating".into(), template: "What makes {symptom} worse?".into(),
            oldcarts_dimension: Some(Aggravating), system_name: None },
        QuestionTemplate { id: "ask_alleviating".into(), template: "What makes {symptom} better?".into(),
            oldcarts_dimension: Some(Alleviating), system_name: None },
        QuestionTemplate { id: "ask_radiation".into(), template: "Does {symptom} spread to other areas?".into(),
            oldcarts_dimension: Some(Radiation), system_name: None },
        QuestionTemplate { id: "ask_timing".into(), template: "Is there a pattern to when {symptom} happens?".into(),
            oldcarts_dimension: Some(Timing), system_name: None },
        QuestionTemplate { id: "ask_severity".into(), template: "On a scale of 1 to 10, how would you rate {symptom}?".into(),
            oldcarts_dimension: Some(Severity), system_name: None },
        QuestionTemplate { id: "clarify_onset".into(), template: "Was this days, weeks, or months ago?".into(),
            oldcarts_dimension: Some(Onset), system_name: None },
        QuestionTemplate { id: "clarify_character".into(), template: "Is it more of an ache or a sharp pain?".into(),
            oldcarts_dimension: Some(Character), system_name: None },
        QuestionTemplate { id: "ask_prescriptions".into(),
            template: "Are you currently taking any prescription medications?".into(),
            oldcarts_dimension: None, system_name: None },
        QuestionTemplate { id: "ask_health_conditions".into(),
            template: "Do you have any health conditions I should know about?".into(),
            oldcarts_dimension: None, system_name: None },
        QuestionTemplate { id: "review_nervous".into(),
            template: "Have you noticed any tingling, numbness, or changes in sensation?".into(),
            oldcarts_dimension: None, system_name: Some("nervous system".into()) },
        QuestionTemplate { id: "review_musculoskeletal".into(),
            template: "Any joint stiffness, muscle weakness, or body aches?".into(),
            oldcarts_dimension: None, system_name: Some("musculoskeletal system".into()) },
        QuestionTemplate { id: "review_digestive".into(),
            template: "Any changes in digestion — nausea, bloating, or bowel changes?".into(),
            oldcarts_dimension: None, system_name: Some("digestive system".into()) },
        QuestionTemplate { id: "review_endocrine".into(),
            template: "Have you noticed changes in energy, weight, or temperature sensitivity?".into(),
            oldcarts_dimension: None, system_name: Some("endocrine system".into()) },
        QuestionTemplate { id: "causation_notice".into(),
            template: "I want to mention — some of your symptoms can be associated with supplements or medications.".into(),
            oldcarts_dimension: None, system_name: None },
    ];

    let clusters = vec![
        SymptomCluster { id: "electrolyte_deficiency".into(), name: "Electrolyte Deficiency Pattern".into(),
            description: "Muscle cramps + insomnia + fatigue suggest electrolyte imbalance.".into(),
            member_symptoms: vec!["muscle_cramps".into(), "insomnia".into(), "fatigue".into()],
            prioritized_systems: vec!["nervous system".into(), "musculoskeletal system".into()] },
        SymptomCluster { id: "stress_response".into(), name: "Stress Response Pattern".into(),
            description: "Anxiety + insomnia suggest chronic stress response.".into(),
            member_symptoms: vec!["anxiety".into(), "insomnia".into()],
            prioritized_systems: vec!["nervous system".into(), "endocrine system".into()] },
    ];

    PgIntakeStore::from_parts(archetypes, profiles, questions, clusters)
}

