// ---------------------------------------------------------------------------
// Intake Knowledge Graph — Type System
//
// Process graph that encodes how to conduct a clinical supplement intake.
// The LLM is the renderer; the graph drives all reasoning.
// ---------------------------------------------------------------------------

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Node types
// ---------------------------------------------------------------------------

/// Phases of the intake interview.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IntakeStageId {
    ChiefComplaint,
    Hpi,
    SystemReview,
    Differentiation,
    CausationInquiry,
    PreRecommendation,
    Recommendation,
    FollowUp,
}

impl IntakeStageId {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ChiefComplaint => "chief_complaint",
            Self::Hpi => "hpi",
            Self::SystemReview => "system_review",
            Self::Differentiation => "differentiation",
            Self::CausationInquiry => "causation_inquiry",
            Self::PreRecommendation => "pre_recommendation",
            Self::Recommendation => "recommendation",
            Self::FollowUp => "follow_up",
        }
    }
}

/// A stage of the intake interview.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntakeStage {
    pub id: IntakeStageId,
    pub description: String,
}

/// A parameterized question the agent can ask.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionTemplate {
    pub id: String,
    /// Template text with {placeholders}.
    pub template: String,
    /// Which OLDCARTS dimension this question targets, if any.
    pub oldcarts_dimension: Option<OldcartsDimension>,
}

/// What information we're trying to gather.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClinicalGoal {
    pub id: String,
    pub description: String,
    /// Which extractor field fulfills this goal.
    pub fulfilled_by: Option<ExtractorField>,
}

/// When to leave a stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExitCondition {
    pub id: String,
    pub description: String,
    pub condition: ExitConditionType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExitConditionType {
    /// Enough OLDCARTS dimensions filled for the active symptom profile.
    OldcartsSufficient,
    /// User is disengaged (short answers, dismissive).
    UserDisengaged,
    /// Candidate confidence is high enough to recommend.
    CandidatesConfident,
    /// All relevant systems have been reviewed.
    SystemsReviewed,
    /// No more differentiating questions available.
    NoDifferentiators,
    /// User explicitly says they're done sharing.
    DoneSharing,
    /// At least one chief complaint has been recorded.
    HasChiefComplaint,
    /// Medication check has been completed.
    MedicationCheckDone,
}

/// Intake-specific knowledge about a symptom, inherits from ArchetypeProfile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymptomProfile {
    pub id: String,
    /// Canonical symptom name (matches supplement KG Symptom node).
    pub name: String,
    /// UMLS CUI from iDISK for cross-source resolution.
    pub cui: Option<String>,
    /// Aliases from iDISK MSKCC for concept mapping.
    pub aliases: Vec<String>,
    /// Which archetype this inherits from.
    pub archetype_id: String,
    /// Overrides to the archetype's relevant OLDCARTS dimensions.
    pub relevant_oldcarts_override: Option<Vec<OldcartsDimension>>,
    /// Overrides to the archetype's irrelevant OLDCARTS dimensions.
    pub irrelevant_oldcarts_override: Option<Vec<OldcartsDimension>>,
    /// Override for sufficient dimension count.
    pub sufficient_dimensions_override: Option<u8>,
    /// Body systems to probe during ROS for this symptom.
    pub associated_systems: Vec<String>,
}

/// Category template grouping symptoms with similar interview logic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchetypeProfile {
    pub id: String,
    pub name: String,
    pub relevant_oldcarts: Vec<OldcartsDimension>,
    pub irrelevant_oldcarts: Vec<OldcartsDimension>,
    pub sufficient_dimensions: u8,
    /// Default systems to probe in ROS.
    pub default_systems: Vec<String>,
}

/// Co-occurring symptom pattern pointing to common underlying cause.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymptomCluster {
    pub id: String,
    pub name: String,
    /// Description of the pattern.
    pub description: String,
    /// Symptom profile IDs that participate in this cluster.
    pub member_symptoms: Vec<String>,
    /// Systems to prioritize when this cluster is detected.
    pub prioritized_systems: Vec<String>,
}

/// What to ask when probing a body system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemReviewNode {
    pub id: String,
    /// System name (matches supplement KG System node).
    pub system_name: String,
    /// Plain-language screening questions.
    pub screening_questions: Vec<String>,
}

/// A step that queries the supplement KG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphActionNode {
    pub id: String,
    pub action_type: GraphActionType,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GraphActionType {
    /// Score candidates from supplement KG.
    QueryCandidates,
    /// Find discriminating questions between top candidates.
    FindDiscriminators,
    /// Check user's medications against interaction edges.
    CheckInteractions,
    /// Check if user's symptoms are adverse reactions to something they take.
    CheckAdverseReactions,
    /// Fetch Mechanism of Action text for recommendation framing.
    FetchMechanism,
    /// Find adjacent systems for ROS.
    FindAdjacentSystems,
}

// ---------------------------------------------------------------------------
// Edge types
// ---------------------------------------------------------------------------

/// All edge types in the intake graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IntakeEdgeType {
    /// IntakeStage → IntakeStage
    HasStage,
    /// ClinicalGoal → QuestionTemplate
    Asks,
    /// ExtractorField → ClinicalGoal (when extraction satisfies a goal)
    Fulfills,
    /// QuestionTemplate → QuestionTemplate (rephrase on failed match)
    FallsBack,
    /// QuestionTemplate → SymptomProfile (question applies to this symptom)
    RelevantFor,
    /// QuestionTemplate → SymptomProfile (skip for this symptom)
    IrrelevantFor,
    /// SymptomProfile → ArchetypeProfile
    BelongsTo,
    /// SymptomProfile → SymptomCluster
    CoOccurs,
    /// SymptomCluster → SystemReviewNode
    Suggests,
    /// ExtractorField → GraphAction (extraction triggers supplement KG query)
    Triggers,
    /// IntakeStage → ExitCondition
    ExitsWhen,
    /// ExitCondition → IntakeStage
    EscalatesTo,
    /// SystemReviewNode → QuestionTemplate
    Probes,
    /// IntakeStage → ClinicalGoal (goals active during this stage)
    HasGoal,
}

/// Metadata on every intake edge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntakeEdgeMeta {
    /// Base traversal priority (0.0–1.0).
    pub priority: f64,
    /// Must this edge be traversed?
    pub required: bool,
    /// Engine CANNOT skip this edge — non-bypassable.
    pub safety_gate: bool,
    /// Max times to probe this goal before moving on.
    pub max_asks: u8,
    /// Optional condition for traversal.
    pub condition: Option<String>,
}

impl Default for IntakeEdgeMeta {
    fn default() -> Self {
        Self {
            priority: 0.5,
            required: false,
            safety_gate: false,
            max_asks: 2,
            condition: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Shared enums
// ---------------------------------------------------------------------------

/// OLDCARTS dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OldcartsDimension {
    Onset,
    Location,
    Duration,
    Character,
    Aggravating,
    Alleviating,
    Radiation,
    Timing,
    Severity,
}

impl OldcartsDimension {
    pub fn all() -> &'static [Self] {
        &[
            Self::Onset,
            Self::Location,
            Self::Duration,
            Self::Character,
            Self::Aggravating,
            Self::Alleviating,
            Self::Radiation,
            Self::Timing,
            Self::Severity,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Onset => "onset",
            Self::Location => "location",
            Self::Duration => "duration",
            Self::Character => "character",
            Self::Aggravating => "aggravating",
            Self::Alleviating => "alleviating",
            Self::Radiation => "radiation",
            Self::Timing => "timing",
            Self::Severity => "severity",
        }
    }
}

/// Extractor output fields that can fulfill clinical goals.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExtractorField {
    Symptom,
    System,
    Onset,
    Location,
    Duration,
    Character,
    Aggravating,
    Alleviating,
    Radiation,
    Timing,
    Severity,
    DeniedSystem,
    Medication,
    Engagement,
}

// ---------------------------------------------------------------------------
// Resolved types — what the traversal engine produces each turn
// ---------------------------------------------------------------------------

/// The resolved action for a single turn, produced by the traversal engine.
#[derive(Debug, Clone)]
pub struct TurnAction {
    /// The question to ask (template filled with session context).
    pub question: Option<ResolvedQuestion>,
    /// Graph actions to fire this turn (supplement KG queries).
    pub graph_actions: Vec<GraphActionType>,
    /// Whether we should transition to a new stage.
    pub next_stage: Option<IntakeStageId>,
    /// Debug trace of the traversal for logging.
    pub trace: Vec<String>,
}

/// A question selected by the traversal engine.
#[derive(Debug, Clone)]
pub struct ResolvedQuestion {
    /// The question template ID.
    pub template_id: String,
    /// The filled question text.
    pub text: String,
    /// Which clinical goal this serves.
    pub goal_id: String,
    /// EIG score that selected this question.
    pub score: f64,
}

/// Effective priority computation inputs, for debugging.
#[derive(Debug, Clone)]
pub struct EigScore {
    pub base_priority: f64,
    pub information_gain: f64,
    pub system_relevance: f64,
    pub effective: f64,
}
