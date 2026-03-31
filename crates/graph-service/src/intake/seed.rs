// ---------------------------------------------------------------------------
// Seed data for the intake knowledge graph.
//
// Populates the core structure: stages, archetypes, goals, questions,
// exit conditions, system reviews, graph actions, and all edges.
// ---------------------------------------------------------------------------

use super::store::IntakeGraphStore;
use super::types::*;

/// Seed the intake graph with core structural data.
/// Idempotent — SurrealDB create-with-key silently skips duplicates.
pub async fn seed_intake_graph(store: &IntakeGraphStore) {
    seed_stages(store).await;
    seed_archetypes(store).await;
    seed_symptom_profiles(store).await;
    seed_goals(store).await;
    seed_questions(store).await;
    seed_exit_conditions(store).await;
    seed_system_reviews(store).await;
    seed_graph_actions(store).await;
    seed_clusters(store).await;
    seed_edges(store).await;
}

// ---------------------------------------------------------------------------
// Stages
// ---------------------------------------------------------------------------

async fn seed_stages(store: &IntakeGraphStore) {
    let stages = vec![
        IntakeStage {
            id: IntakeStageId::ChiefComplaint,
            description: "Gather what brings the user in today.".into(),
        },
        IntakeStage {
            id: IntakeStageId::Hpi,
            description: "Deep-dive on each chief complaint using relevant OLDCARTS dimensions.".into(),
        },
        IntakeStage {
            id: IntakeStageId::SystemReview,
            description: "Graph-guided body system sweep, recording pertinent negatives.".into(),
        },
        IntakeStage {
            id: IntakeStageId::Differentiation,
            description: "Ask discriminating questions to narrow supplement candidates.".into(),
        },
        IntakeStage {
            id: IntakeStageId::CausationInquiry,
            description: "Investigate whether current supplements/medications may be causing symptoms.".into(),
        },
        IntakeStage {
            id: IntakeStageId::Recommendation,
            description: "Present research-backed supplement suggestions with evidence.".into(),
        },
    ];
    for s in &stages {
        store.add_stage(s).await;
    }
}

// ---------------------------------------------------------------------------
// Archetypes (~10 category templates)
// ---------------------------------------------------------------------------

async fn seed_archetypes(store: &IntakeGraphStore) {
    use OldcartsDimension::*;

    let archetypes = vec![
        ArchetypeProfile {
            id: "pain".into(),
            name: "Pain".into(),
            relevant_oldcarts: vec![Onset, Location, Character, Severity, Aggravating, Timing],
            irrelevant_oldcarts: vec![],
            sufficient_dimensions: 4,
            default_systems: vec!["musculoskeletal system".into(), "nervous system".into()],
        },
        ArchetypeProfile {
            id: "sleep".into(),
            name: "Sleep".into(),
            relevant_oldcarts: vec![Onset, Duration, Timing, Aggravating, Alleviating],
            irrelevant_oldcarts: vec![Location, Radiation],
            sufficient_dimensions: 3,
            default_systems: vec!["nervous system".into(), "endocrine system".into()],
        },
        ArchetypeProfile {
            id: "mood".into(),
            name: "Mood".into(),
            relevant_oldcarts: vec![Onset, Duration, Character, Severity, Timing],
            irrelevant_oldcarts: vec![Location, Radiation],
            sufficient_dimensions: 3,
            default_systems: vec!["nervous system".into(), "endocrine system".into()],
        },
        ArchetypeProfile {
            id: "digestive".into(),
            name: "Digestive".into(),
            relevant_oldcarts: vec![Onset, Location, Character, Timing, Aggravating],
            irrelevant_oldcarts: vec![Radiation],
            sufficient_dimensions: 4,
            default_systems: vec!["digestive system".into(), "immune system".into()],
        },
        ArchetypeProfile {
            id: "fatigue".into(),
            name: "Fatigue / Energy".into(),
            relevant_oldcarts: vec![Onset, Duration, Severity, Timing, Aggravating],
            irrelevant_oldcarts: vec![Location, Radiation, Character],
            sufficient_dimensions: 3,
            default_systems: vec![
                "endocrine system".into(),
                "immune system".into(),
                "nervous system".into(),
            ],
        },
        ArchetypeProfile {
            id: "skin".into(),
            name: "Skin / Integumentary".into(),
            relevant_oldcarts: vec![Onset, Location, Character, Aggravating],
            irrelevant_oldcarts: vec![Radiation, Timing],
            sufficient_dimensions: 3,
            default_systems: vec!["immune system".into(), "integumentary system".into()],
        },
        ArchetypeProfile {
            id: "respiratory".into(),
            name: "Respiratory".into(),
            relevant_oldcarts: vec![Onset, Character, Timing, Aggravating, Severity],
            irrelevant_oldcarts: vec![Location, Radiation],
            sufficient_dimensions: 3,
            default_systems: vec!["respiratory system".into(), "immune system".into()],
        },
        ArchetypeProfile {
            id: "cardiovascular".into(),
            name: "Cardiovascular".into(),
            relevant_oldcarts: vec![Onset, Character, Timing, Severity, Radiation],
            irrelevant_oldcarts: vec![],
            sufficient_dimensions: 4,
            default_systems: vec!["cardiovascular system".into(), "nervous system".into()],
        },
        ArchetypeProfile {
            id: "immune".into(),
            name: "Immune / Inflammatory".into(),
            relevant_oldcarts: vec![Onset, Duration, Timing, Severity],
            irrelevant_oldcarts: vec![Radiation, Character],
            sufficient_dimensions: 3,
            default_systems: vec!["immune system".into()],
        },
        ArchetypeProfile {
            id: "cognitive".into(),
            name: "Cognitive / Neurological".into(),
            relevant_oldcarts: vec![Onset, Duration, Character, Timing, Severity],
            irrelevant_oldcarts: vec![Radiation],
            sufficient_dimensions: 3,
            default_systems: vec!["nervous system".into()],
        },
    ];
    for a in &archetypes {
        store.add_archetype(a).await;
    }
}

// ---------------------------------------------------------------------------
// Symptom Profiles — one per common presenting complaint, with aliases
// so free-text matching (e.g. "headaches" → "headache" profile) works.
// ---------------------------------------------------------------------------

async fn seed_symptom_profiles(store: &IntakeGraphStore) {
    let profiles = vec![
        // --- Pain archetype ---
        SymptomProfile {
            id: "headache".into(),
            name: "Headache".into(),
            cui: Some("C0018681".into()),
            aliases: vec!["headaches".into(), "head pain".into(), "head ache".into(),
                          "migraine".into(), "migraines".into(), "tension headache".into(),
                          "occipital headache".into(), "occipital tension".into(),
                          "tension headaches".into()],
            archetype_id: "cognitive".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: Some(3),
            associated_systems: vec!["nervous system".into()],
        },
        SymptomProfile {
            id: "muscle_cramps".into(),
            name: "Muscle Cramps".into(),
            cui: Some("C0026821".into()),
            aliases: vec!["cramps".into(), "muscle cramp".into(), "cramping".into(),
                          "muscle spasms".into(), "spasms".into(), "leg cramps".into(),
                          "muscle tightness".into(), "muscle tension".into(), "tightness".into()],
            archetype_id: "pain".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: Some(3),
            associated_systems: vec!["musculoskeletal system".into(), "nervous system".into()],
        },
        SymptomProfile {
            id: "joint_pain".into(),
            name: "Joint Pain".into(),
            cui: Some("C0003862".into()),
            aliases: vec!["joint ache".into(), "arthralgia".into(), "stiff joints".into(),
                          "joint stiffness".into(), "achy joints".into(), "joint discomfort".into()],
            archetype_id: "pain".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: None,
            associated_systems: vec!["musculoskeletal system".into(), "immune system".into()],
        },
        SymptomProfile {
            id: "back_pain".into(),
            name: "Back Pain".into(),
            cui: Some("C0004604".into()),
            aliases: vec!["backache".into(), "back ache".into(), "lower back pain".into(),
                          "upper back pain".into(), "back discomfort".into(), "spinal pain".into(),
                          "subscapular".into(), "subscapular tightness".into()],
            archetype_id: "pain".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: None,
            associated_systems: vec!["musculoskeletal system".into()],
        },
        // --- Sleep archetype ---
        SymptomProfile {
            id: "insomnia".into(),
            name: "Insomnia".into(),
            cui: Some("C0917801".into()),
            aliases: vec!["trouble sleeping".into(), "can't sleep".into(), "sleep problems".into(),
                          "poor sleep".into(), "sleep issues".into(), "difficulty sleeping".into(),
                          "waking up at night".into(), "sleep disturbance".into()],
            archetype_id: "sleep".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: Some(3),
            associated_systems: vec!["nervous system".into(), "endocrine system".into()],
        },
        // --- Fatigue archetype ---
        SymptomProfile {
            id: "fatigue".into(),
            name: "Fatigue".into(),
            cui: Some("C0015672".into()),
            aliases: vec!["tired".into(), "tiredness".into(), "exhaustion".into(), "exhausted".into(),
                          "low energy".into(), "lack of energy".into(), "no energy".into(),
                          "always tired".into(), "chronic fatigue".into(), "lethargy".into(),
                          "sluggish".into(), "worn out".into()],
            archetype_id: "fatigue".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: Some(3),
            associated_systems: vec!["endocrine system".into(), "nervous system".into()],
        },
        // --- Digestive archetype ---
        SymptomProfile {
            id: "nausea".into(),
            name: "Nausea".into(),
            cui: Some("C0027497".into()),
            aliases: vec!["nauseous".into(), "sick to stomach".into(), "upset stomach".into(),
                          "queasiness".into(), "queasy".into(), "stomach upset".into(),
                          "gut upset".into(), "GI upset".into(), "gut troubles".into(),
                          "GI allergy upset".into()],
            archetype_id: "digestive".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: Some(3),
            associated_systems: vec!["digestive system".into()],
        },
        SymptomProfile {
            id: "bloating".into(),
            name: "Bloating".into(),
            cui: Some("C0000731".into()),
            aliases: vec!["bloated".into(), "abdominal bloating".into(), "gas".into(),
                          "gassy".into(), "distension".into(), "fullness".into(),
                          "digestive discomfort".into(), "gut discomfort".into()],
            archetype_id: "digestive".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: Some(3),
            associated_systems: vec!["digestive system".into()],
        },
        SymptomProfile {
            id: "digestive_discomfort".into(),
            name: "Digestive Discomfort".into(),
            cui: None,
            aliases: vec!["stomach pain".into(), "abdominal pain".into(), "gut pain".into(),
                          "stomach ache".into(), "tummy ache".into(), "belly pain".into(),
                          "indigestion".into(), "GI issues".into(), "gastrointestinal issues".into(),
                          "gut issues".into(), "gut troubles".into(), "stomach troubles".into()],
            archetype_id: "digestive".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: Some(3),
            associated_systems: vec!["digestive system".into()],
        },
        // --- Mood archetype ---
        SymptomProfile {
            id: "anxiety".into(),
            name: "Anxiety".into(),
            cui: Some("C0003469".into()),
            aliases: vec!["anxious".into(), "worried".into(), "worry".into(), "stress".into(),
                          "stressed".into(), "nervous".into(), "on edge".into(),
                          "panic".into(), "panic attacks".into()],
            archetype_id: "mood".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: Some(3),
            associated_systems: vec!["nervous system".into(), "endocrine system".into()],
        },
        SymptomProfile {
            id: "depression".into(),
            name: "Depression".into(),
            cui: Some("C0011570".into()),
            aliases: vec!["depressed".into(), "low mood".into(), "sad".into(), "sadness".into(),
                          "down".into(), "feeling down".into(), "hopeless".into(), "apathy".into()],
            archetype_id: "mood".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: Some(3),
            associated_systems: vec!["nervous system".into(), "endocrine system".into()],
        },
        // --- Cognitive archetype ---
        SymptomProfile {
            id: "brain_fog".into(),
            name: "Brain Fog".into(),
            cui: None,
            aliases: vec!["foggy brain".into(), "mental fog".into(), "fuzzy thinking".into(),
                          "can't concentrate".into(), "trouble concentrating".into(),
                          "poor focus".into(), "memory problems".into(), "forgetful".into(),
                          "cognitive issues".into()],
            archetype_id: "cognitive".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: Some(3),
            associated_systems: vec!["nervous system".into()],
        },
        // --- Immune/Inflammatory archetype ---
        SymptomProfile {
            id: "inflammation".into(),
            name: "Inflammation".into(),
            cui: Some("C0021368".into()),
            aliases: vec!["inflamed".into(), "swelling".into(), "swollen".into(), "redness".into(),
                          "hot joints".into(), "inflammatory pain".into(), "flare".into()],
            archetype_id: "immune".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: Some(3),
            associated_systems: vec!["immune system".into()],
        },
        // --- Skin archetype ---
        SymptomProfile {
            id: "skin_issues".into(),
            name: "Skin Issues".into(),
            cui: None,
            aliases: vec!["rash".into(), "dry skin".into(), "itchy skin".into(), "eczema".into(),
                          "skin irritation".into(), "acne".into(), "breakouts".into(),
                          "psoriasis".into(), "hives".into(), "skin rash".into()],
            archetype_id: "skin".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: Some(3),
            associated_systems: vec!["immune system".into(), "integumentary system".into()],
        },
        // --- Other common complaints ---
        SymptomProfile {
            id: "cold_intolerance".into(),
            name: "Cold Intolerance".into(),
            cui: None,
            aliases: vec!["always cold".into(), "sensitive to cold".into(),
                          "cold sensitivity".into(), "feel cold".into()],
            archetype_id: "fatigue".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: Some(2),
            associated_systems: vec!["endocrine system".into()],
        },
        SymptomProfile {
            id: "hair_thinning".into(),
            name: "Hair Thinning".into(),
            cui: None,
            aliases: vec!["hair loss".into(), "thinning hair".into(), "hair fall".into(),
                          "alopecia".into(), "balding".into()],
            archetype_id: "immune".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: Some(2),
            associated_systems: vec!["endocrine system".into(), "integumentary system".into()],
        },
        SymptomProfile {
            id: "muscle_tension".into(),
            name: "Muscle Tension".into(),
            cui: None,
            aliases: vec!["tense muscles".into(), "tight muscles".into(), "stiffness".into(),
                          "muscle stiffness".into(), "neck tension".into(), "shoulder tension".into()],
            archetype_id: "pain".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: Some(3),
            associated_systems: vec!["musculoskeletal system".into(), "nervous system".into()],
        },
    ];

    for p in &profiles {
        store.add_symptom_profile(p).await;
    }
}

// ---------------------------------------------------------------------------
// Clinical Goals
// ---------------------------------------------------------------------------

async fn seed_goals(store: &IntakeGraphStore) {
    let goals = vec![
        // OLDCARTS goals
        ClinicalGoal {
            id: "characterize_onset".into(),
            description: "When did this start?".into(),
            fulfilled_by: Some(ExtractorField::Onset),
        },
        ClinicalGoal {
            id: "characterize_location".into(),
            description: "Where is it?".into(),
            fulfilled_by: Some(ExtractorField::Location),
        },
        ClinicalGoal {
            id: "characterize_duration".into(),
            description: "How long does it last?".into(),
            fulfilled_by: Some(ExtractorField::Duration),
        },
        ClinicalGoal {
            id: "characterize_character".into(),
            description: "What does it feel like?".into(),
            fulfilled_by: Some(ExtractorField::Character),
        },
        ClinicalGoal {
            id: "characterize_aggravating".into(),
            description: "What makes it worse?".into(),
            fulfilled_by: Some(ExtractorField::Aggravating),
        },
        ClinicalGoal {
            id: "characterize_alleviating".into(),
            description: "What makes it better?".into(),
            fulfilled_by: Some(ExtractorField::Alleviating),
        },
        ClinicalGoal {
            id: "characterize_radiation".into(),
            description: "Does it spread?".into(),
            fulfilled_by: Some(ExtractorField::Radiation),
        },
        ClinicalGoal {
            id: "characterize_timing".into(),
            description: "When does it happen?".into(),
            fulfilled_by: Some(ExtractorField::Timing),
        },
        ClinicalGoal {
            id: "characterize_severity".into(),
            description: "How bad is it (1-10)?".into(),
            fulfilled_by: Some(ExtractorField::Severity),
        },
        // Non-OLDCARTS goals
        ClinicalGoal {
            id: "identify_chief_complaint".into(),
            description: "What brings you in today?".into(),
            fulfilled_by: Some(ExtractorField::Symptom),
        },
        ClinicalGoal {
            id: "check_medications".into(),
            description: "Currently taking any prescriptions or supplements?".into(),
            fulfilled_by: Some(ExtractorField::Medication),
        },
        ClinicalGoal {
            id: "identify_system_involvement".into(),
            description: "Probe for body system involvement.".into(),
            fulfilled_by: Some(ExtractorField::System),
        },
        ClinicalGoal {
            id: "record_denial".into(),
            description: "Record that user denies symptoms in a system.".into(),
            fulfilled_by: Some(ExtractorField::DeniedSystem),
        },
    ];
    for g in &goals {
        store.add_goal(g).await;
    }
}

// ---------------------------------------------------------------------------
// Question Templates
// ---------------------------------------------------------------------------

async fn seed_questions(store: &IntakeGraphStore) {
    let questions = vec![
        QuestionTemplate {
            id: "what_brings_you_in".into(),
            template: "What brings you in today?".into(),
            oldcarts_dimension: None,
        },
        QuestionTemplate {
            id: "anything_else".into(),
            template: "Is there anything else bothering you?".into(),
            oldcarts_dimension: None,
        },
        // OLDCARTS templates
        QuestionTemplate {
            id: "ask_onset".into(),
            template: "When did {symptom} start?".into(),
            oldcarts_dimension: Some(OldcartsDimension::Onset),
        },
        QuestionTemplate {
            id: "ask_location".into(),
            template: "Where exactly do you feel {symptom}?".into(),
            oldcarts_dimension: Some(OldcartsDimension::Location),
        },
        QuestionTemplate {
            id: "ask_duration".into(),
            template: "How long does {symptom} typically last?".into(),
            oldcarts_dimension: Some(OldcartsDimension::Duration),
        },
        QuestionTemplate {
            id: "ask_character".into(),
            template: "What does {symptom} feel like?".into(),
            oldcarts_dimension: Some(OldcartsDimension::Character),
        },
        QuestionTemplate {
            id: "ask_aggravating".into(),
            template: "What makes {symptom} worse?".into(),
            oldcarts_dimension: Some(OldcartsDimension::Aggravating),
        },
        QuestionTemplate {
            id: "ask_alleviating".into(),
            template: "What makes {symptom} better?".into(),
            oldcarts_dimension: Some(OldcartsDimension::Alleviating),
        },
        QuestionTemplate {
            id: "ask_radiation".into(),
            template: "Does {symptom} spread to other areas?".into(),
            oldcarts_dimension: Some(OldcartsDimension::Radiation),
        },
        QuestionTemplate {
            id: "ask_timing".into(),
            template: "Is there a pattern to when {symptom} happens?".into(),
            oldcarts_dimension: Some(OldcartsDimension::Timing),
        },
        QuestionTemplate {
            id: "ask_severity".into(),
            template: "On a scale of 1 to 10, how would you rate {symptom}?".into(),
            oldcarts_dimension: Some(OldcartsDimension::Severity),
        },
        // Fallback / clarification
        QuestionTemplate {
            id: "clarify_onset".into(),
            template: "Just to make sure I understand — was this days, weeks, or months ago?".into(),
            oldcarts_dimension: Some(OldcartsDimension::Onset),
        },
        QuestionTemplate {
            id: "clarify_character".into(),
            template: "Is it more of an ache, a sharp pain, cramping, or something else?".into(),
            oldcarts_dimension: Some(OldcartsDimension::Character),
        },
        // Medication check
        QuestionTemplate {
            id: "ask_medications".into(),
            template: "Are you currently taking any prescription medications or other supplements?".into(),
            oldcarts_dimension: None,
        },
        // System review templates
        QuestionTemplate {
            id: "review_nervous".into(),
            template: "Have you noticed any tingling, numbness, or changes in sensation?".into(),
            oldcarts_dimension: None,
        },
        QuestionTemplate {
            id: "review_digestive".into(),
            template: "Any changes in digestion — nausea, bloating, or bowel changes?".into(),
            oldcarts_dimension: None,
        },
        QuestionTemplate {
            id: "review_endocrine".into(),
            template: "Have you noticed changes in energy, weight, or temperature sensitivity?".into(),
            oldcarts_dimension: None,
        },
        QuestionTemplate {
            id: "review_musculoskeletal".into(),
            template: "Any joint stiffness, muscle weakness, or body aches?".into(),
            oldcarts_dimension: None,
        },
        QuestionTemplate {
            id: "review_immune".into(),
            template: "Have you been getting sick more often or noticing increased inflammation?".into(),
            oldcarts_dimension: None,
        },
        QuestionTemplate {
            id: "review_cardiovascular".into(),
            template: "Any heart palpitations, dizziness, or shortness of breath?".into(),
            oldcarts_dimension: None,
        },
        QuestionTemplate {
            id: "review_respiratory".into(),
            template: "Any coughing, wheezing, or breathing difficulties?".into(),
            oldcarts_dimension: None,
        },
        QuestionTemplate {
            id: "review_skin".into(),
            template: "Any skin changes — rashes, dryness, or irritation?".into(),
            oldcarts_dimension: None,
        },
        // Ready to hear results
        QuestionTemplate {
            id: "ready_for_results".into(),
            template: "I have some thoughts on what might help. Ready to hear what the research suggests?".into(),
            oldcarts_dimension: None,
        },
        // Causation
        QuestionTemplate {
            id: "causation_notice".into(),
            template: "I want to mention — some of your symptoms can be associated with supplements or medications. Let me factor that into what I share with you.".into(),
            oldcarts_dimension: None,
        },
    ];
    for q in &questions {
        store.add_question(q).await;
    }
}

// ---------------------------------------------------------------------------
// Exit Conditions
// ---------------------------------------------------------------------------

async fn seed_exit_conditions(store: &IntakeGraphStore) {
    let conditions = vec![
        ExitCondition {
            id: "has_chief_complaint".into(),
            description: "At least one chief complaint recorded.".into(),
            condition: ExitConditionType::HasChiefComplaint,
        },
        ExitCondition {
            id: "oldcarts_sufficient".into(),
            description: "Enough OLDCARTS dimensions filled for active symptom profiles.".into(),
            condition: ExitConditionType::OldcartsSufficient,
        },
        ExitCondition {
            id: "user_disengaged".into(),
            description: "User is giving short, dismissive answers.".into(),
            condition: ExitConditionType::UserDisengaged,
        },
        ExitCondition {
            id: "candidates_confident".into(),
            description: "High confidence gap between top candidates.".into(),
            condition: ExitConditionType::CandidatesConfident,
        },
        ExitCondition {
            id: "systems_reviewed".into(),
            description: "All relevant systems have been reviewed.".into(),
            condition: ExitConditionType::SystemsReviewed,
        },
        ExitCondition {
            id: "no_differentiators".into(),
            description: "No more differentiating questions available.".into(),
            condition: ExitConditionType::NoDifferentiators,
        },
        ExitCondition {
            id: "done_sharing".into(),
            description: "User explicitly says they're done.".into(),
            condition: ExitConditionType::DoneSharing,
        },
        ExitCondition {
            id: "medication_check_done".into(),
            description: "User has been asked about medications.".into(),
            condition: ExitConditionType::MedicationCheckDone,
        },
    ];
    for ec in &conditions {
        store.add_exit_condition(ec).await;
    }
}

// ---------------------------------------------------------------------------
// System Reviews
// ---------------------------------------------------------------------------

async fn seed_system_reviews(store: &IntakeGraphStore) {
    let reviews = vec![
        SystemReviewNode {
            id: "sr_nervous".into(),
            system_name: "nervous system".into(),
            screening_questions: vec![
                "Have you noticed any tingling, numbness, or changes in sensation?".into(),
                "Any headaches, dizziness, or trouble concentrating?".into(),
            ],
        },
        SystemReviewNode {
            id: "sr_digestive".into(),
            system_name: "digestive system".into(),
            screening_questions: vec![
                "Any changes in digestion — nausea, bloating, or bowel changes?".into(),
                "Any appetite changes or discomfort after eating?".into(),
            ],
        },
        SystemReviewNode {
            id: "sr_endocrine".into(),
            system_name: "endocrine system".into(),
            screening_questions: vec![
                "Have you noticed changes in energy, weight, or temperature sensitivity?".into(),
                "Any mood swings or changes in sleep patterns?".into(),
            ],
        },
        SystemReviewNode {
            id: "sr_musculoskeletal".into(),
            system_name: "musculoskeletal system".into(),
            screening_questions: vec![
                "Any joint stiffness, muscle weakness, or body aches?".into(),
                "Any changes in mobility or range of motion?".into(),
            ],
        },
        SystemReviewNode {
            id: "sr_immune".into(),
            system_name: "immune system".into(),
            screening_questions: vec![
                "Have you been getting sick more often?".into(),
                "Any signs of increased inflammation — swelling, redness?".into(),
            ],
        },
        SystemReviewNode {
            id: "sr_cardiovascular".into(),
            system_name: "cardiovascular system".into(),
            screening_questions: vec![
                "Any heart palpitations, dizziness, or shortness of breath?".into(),
            ],
        },
        SystemReviewNode {
            id: "sr_respiratory".into(),
            system_name: "respiratory system".into(),
            screening_questions: vec![
                "Any coughing, wheezing, or breathing difficulties?".into(),
            ],
        },
        SystemReviewNode {
            id: "sr_skin".into(),
            system_name: "integumentary system".into(),
            screening_questions: vec![
                "Any skin changes — rashes, dryness, or irritation?".into(),
            ],
        },
    ];
    for sr in &reviews {
        store.add_system_review(sr).await;
    }
}

// ---------------------------------------------------------------------------
// Graph Actions
// ---------------------------------------------------------------------------

async fn seed_graph_actions(store: &IntakeGraphStore) {
    let actions = vec![
        GraphActionNode {
            id: "ga_query_candidates".into(),
            action_type: GraphActionType::QueryCandidates,
            description: "Score candidates from supplement KG.".into(),
        },
        GraphActionNode {
            id: "ga_find_discriminators".into(),
            action_type: GraphActionType::FindDiscriminators,
            description: "Find differentiating questions between top candidates.".into(),
        },
        GraphActionNode {
            id: "ga_check_interactions".into(),
            action_type: GraphActionType::CheckInteractions,
            description: "Check medications against interaction edges.".into(),
        },
        GraphActionNode {
            id: "ga_check_adverse_reactions".into(),
            action_type: GraphActionType::CheckAdverseReactions,
            description: "Check if symptoms are adverse reactions to current supplements.".into(),
        },
        GraphActionNode {
            id: "ga_fetch_mechanism".into(),
            action_type: GraphActionType::FetchMechanism,
            description: "Fetch Mechanism of Action text for recommendations.".into(),
        },
        GraphActionNode {
            id: "ga_find_adjacent_systems".into(),
            action_type: GraphActionType::FindAdjacentSystems,
            description: "Find body systems adjacent to current candidates.".into(),
        },
    ];
    for ga in &actions {
        store.add_graph_action(ga).await;
    }
}

// ---------------------------------------------------------------------------
// Symptom Clusters
// ---------------------------------------------------------------------------

async fn seed_clusters(store: &IntakeGraphStore) {
    let clusters = vec![
        SymptomCluster {
            id: "electrolyte_deficiency".into(),
            name: "Electrolyte Deficiency Pattern".into(),
            description: "Muscle cramps + insomnia + fatigue suggest electrolyte imbalance.".into(),
            member_symptoms: vec!["muscle_cramps".into(), "insomnia".into(), "fatigue".into()],
            prioritized_systems: vec!["nervous system".into(), "musculoskeletal system".into()],
        },
        SymptomCluster {
            id: "thyroid_pattern".into(),
            name: "Thyroid Pattern".into(),
            description: "Fatigue + cold intolerance + hair thinning suggest thyroid involvement.".into(),
            member_symptoms: vec!["fatigue".into(), "cold_intolerance".into(), "hair_thinning".into()],
            prioritized_systems: vec!["endocrine system".into()],
        },
        SymptomCluster {
            id: "gut_brain_pattern".into(),
            name: "Gut-Brain Axis Pattern".into(),
            description: "Mood changes + fatigue + digestive issues suggest gut-brain axis involvement.".into(),
            member_symptoms: vec!["depression".into(), "fatigue".into(), "bloating".into()],
            prioritized_systems: vec!["digestive system".into(), "nervous system".into()],
        },
        SymptomCluster {
            id: "stress_response".into(),
            name: "Stress Response Pattern".into(),
            description: "Anxiety + insomnia + muscle tension suggest chronic stress response.".into(),
            member_symptoms: vec!["anxiety".into(), "insomnia".into(), "muscle_tension".into()],
            prioritized_systems: vec!["nervous system".into(), "endocrine system".into()],
        },
        SymptomCluster {
            id: "inflammatory_pattern".into(),
            name: "Systemic Inflammation Pattern".into(),
            description: "Joint pain + fatigue + digestive issues suggest systemic inflammation.".into(),
            member_symptoms: vec!["joint_pain".into(), "fatigue".into(), "digestive_discomfort".into()],
            prioritized_systems: vec!["immune system".into(), "musculoskeletal system".into()],
        },
    ];
    for c in &clusters {
        store.add_cluster(c).await;
    }
}

// ---------------------------------------------------------------------------
// Edges — the wiring that makes the graph a graph
// ---------------------------------------------------------------------------

async fn seed_edges(store: &IntakeGraphStore) {
    // --- Stage transitions ---
    // chief_complaint → hpi
    store.add_edge(
        "chief_complaint", "hpi",
        IntakeEdgeType::HasStage,
        IntakeEdgeMeta { priority: 1.0, required: true, ..Default::default() },
    ).await;

    // hpi → system_review
    store.add_edge(
        "hpi", "system_review",
        IntakeEdgeType::HasStage,
        IntakeEdgeMeta {
            priority: 0.8,
            condition: Some("candidates > 0".into()),
            ..Default::default()
        },
    ).await;

    // hpi → differentiation (skip ROS if confident)
    store.add_edge(
        "hpi", "differentiation",
        IntakeEdgeType::HasStage,
        IntakeEdgeMeta {
            priority: 0.6,
            condition: Some("candidates > 1".into()),
            ..Default::default()
        },
    ).await;

    // system_review → differentiation
    store.add_edge(
        "system_review", "differentiation",
        IntakeEdgeType::HasStage,
        IntakeEdgeMeta {
            priority: 0.8,
            condition: Some("differentiators_available".into()),
            ..Default::default()
        },
    ).await;

    // system_review → recommendation (skip differentiation if clear winner)
    store.add_edge(
        "system_review", "recommendation",
        IntakeEdgeType::HasStage,
        IntakeEdgeMeta {
            priority: 0.5,
            condition: Some("candidates_confident".into()),
            ..Default::default()
        },
    ).await;

    // differentiation → causation_inquiry
    store.add_edge(
        "differentiation", "causation_inquiry",
        IntakeEdgeType::HasStage,
        IntakeEdgeMeta {
            priority: 0.9,
            condition: Some("medications_disclosed".into()),
            ..Default::default()
        },
    ).await;

    // differentiation → recommendation
    store.add_edge(
        "differentiation", "recommendation",
        IntakeEdgeType::HasStage,
        IntakeEdgeMeta { priority: 0.7, ..Default::default() },
    ).await;

    // causation_inquiry → recommendation
    store.add_edge(
        "causation_inquiry", "recommendation",
        IntakeEdgeType::HasStage,
        IntakeEdgeMeta { priority: 1.0, required: true, ..Default::default() },
    ).await;

    // --- Stage → goals ---
    store.add_edge(
        "chief_complaint", "identify_chief_complaint",
        IntakeEdgeType::HasGoal,
        IntakeEdgeMeta { priority: 1.0, required: true, ..Default::default() },
    ).await;

    // HPI goals (OLDCARTS)
    for goal_id in &[
        "characterize_onset", "characterize_location", "characterize_duration",
        "characterize_character", "characterize_aggravating", "characterize_alleviating",
        "characterize_radiation", "characterize_timing", "characterize_severity",
    ] {
        store.add_edge(
            "hpi", goal_id,
            IntakeEdgeType::HasGoal,
            IntakeEdgeMeta { priority: 0.7, ..Default::default() },
        ).await;
    }

    store.add_edge(
        "system_review", "identify_system_involvement",
        IntakeEdgeType::HasGoal,
        IntakeEdgeMeta { priority: 0.8, ..Default::default() },
    ).await;

    store.add_edge(
        "system_review", "record_denial",
        IntakeEdgeType::HasGoal,
        IntakeEdgeMeta { priority: 0.5, ..Default::default() },
    ).await;

    // Medication check — safety gate, non-bypassable
    store.add_edge(
        "hpi", "check_medications",
        IntakeEdgeType::HasGoal,
        IntakeEdgeMeta {
            priority: 0.9,
            required: true,
            safety_gate: true,
            max_asks: 3,
            ..Default::default()
        },
    ).await;

    // --- Goals → questions ---
    store.add_edge("identify_chief_complaint", "what_brings_you_in", IntakeEdgeType::Asks, IntakeEdgeMeta { priority: 1.0, ..Default::default() }).await;
    store.add_edge("identify_chief_complaint", "anything_else", IntakeEdgeType::Asks, IntakeEdgeMeta { priority: 0.5, ..Default::default() }).await;

    store.add_edge("characterize_onset", "ask_onset", IntakeEdgeType::Asks, IntakeEdgeMeta { priority: 0.9, ..Default::default() }).await;
    store.add_edge("characterize_location", "ask_location", IntakeEdgeType::Asks, IntakeEdgeMeta { priority: 0.8, ..Default::default() }).await;
    store.add_edge("characterize_duration", "ask_duration", IntakeEdgeType::Asks, IntakeEdgeMeta { priority: 0.7, ..Default::default() }).await;
    store.add_edge("characterize_character", "ask_character", IntakeEdgeType::Asks, IntakeEdgeMeta { priority: 0.85, ..Default::default() }).await;
    store.add_edge("characterize_aggravating", "ask_aggravating", IntakeEdgeType::Asks, IntakeEdgeMeta { priority: 0.75, ..Default::default() }).await;
    store.add_edge("characterize_alleviating", "ask_alleviating", IntakeEdgeType::Asks, IntakeEdgeMeta { priority: 0.6, ..Default::default() }).await;
    store.add_edge("characterize_radiation", "ask_radiation", IntakeEdgeType::Asks, IntakeEdgeMeta { priority: 0.4, ..Default::default() }).await;
    store.add_edge("characterize_timing", "ask_timing", IntakeEdgeType::Asks, IntakeEdgeMeta { priority: 0.7, ..Default::default() }).await;
    store.add_edge("characterize_severity", "ask_severity", IntakeEdgeType::Asks, IntakeEdgeMeta { priority: 0.65, ..Default::default() }).await;

    store.add_edge("check_medications", "ask_medications", IntakeEdgeType::Asks, IntakeEdgeMeta { priority: 1.0, safety_gate: true, required: true, max_asks: 3, ..Default::default() }).await;

    // --- Fallback edges (rephrasing) ---
    store.add_edge("ask_onset", "clarify_onset", IntakeEdgeType::FallsBack, IntakeEdgeMeta { priority: 0.5, ..Default::default() }).await;
    store.add_edge("ask_character", "clarify_character", IntakeEdgeType::FallsBack, IntakeEdgeMeta { priority: 0.5, ..Default::default() }).await;

    // --- System review → question wiring ---
    store.add_edge("sr_nervous", "review_nervous", IntakeEdgeType::Probes, IntakeEdgeMeta { priority: 0.8, ..Default::default() }).await;
    store.add_edge("sr_digestive", "review_digestive", IntakeEdgeType::Probes, IntakeEdgeMeta { priority: 0.8, ..Default::default() }).await;
    store.add_edge("sr_endocrine", "review_endocrine", IntakeEdgeType::Probes, IntakeEdgeMeta { priority: 0.7, ..Default::default() }).await;
    store.add_edge("sr_musculoskeletal", "review_musculoskeletal", IntakeEdgeType::Probes, IntakeEdgeMeta { priority: 0.7, ..Default::default() }).await;
    store.add_edge("sr_immune", "review_immune", IntakeEdgeType::Probes, IntakeEdgeMeta { priority: 0.6, ..Default::default() }).await;
    store.add_edge("sr_cardiovascular", "review_cardiovascular", IntakeEdgeType::Probes, IntakeEdgeMeta { priority: 0.6, ..Default::default() }).await;
    store.add_edge("sr_respiratory", "review_respiratory", IntakeEdgeType::Probes, IntakeEdgeMeta { priority: 0.5, ..Default::default() }).await;
    store.add_edge("sr_skin", "review_skin", IntakeEdgeType::Probes, IntakeEdgeMeta { priority: 0.5, ..Default::default() }).await;

    // --- Exit conditions ---
    store.add_edge("chief_complaint", "has_chief_complaint", IntakeEdgeType::ExitsWhen, IntakeEdgeMeta::default()).await;
    store.add_edge("has_chief_complaint", "hpi", IntakeEdgeType::EscalatesTo, IntakeEdgeMeta::default()).await;

    store.add_edge("hpi", "oldcarts_sufficient", IntakeEdgeType::ExitsWhen, IntakeEdgeMeta::default()).await;
    store.add_edge("hpi", "user_disengaged", IntakeEdgeType::ExitsWhen, IntakeEdgeMeta::default()).await;
    store.add_edge("hpi", "candidates_confident", IntakeEdgeType::ExitsWhen, IntakeEdgeMeta::default()).await;

    store.add_edge("system_review", "systems_reviewed", IntakeEdgeType::ExitsWhen, IntakeEdgeMeta::default()).await;
    store.add_edge("system_review", "user_disengaged", IntakeEdgeType::ExitsWhen, IntakeEdgeMeta::default()).await;

    store.add_edge("differentiation", "no_differentiators", IntakeEdgeType::ExitsWhen, IntakeEdgeMeta::default()).await;
    store.add_edge("differentiation", "done_sharing", IntakeEdgeType::ExitsWhen, IntakeEdgeMeta::default()).await;
}
