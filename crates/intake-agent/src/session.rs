use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::candidates::CandidateSet;
use graph_service::intake::types::OldcartsDimension;

// ---------------------------------------------------------------------------
// Intake session — tracks a single clinical intake conversation
//
// Persisted per-session (in-memory for v1), NOT stored in the graph.
// The session drives the conversation: what phase we're in, what we've
// gathered, and what candidates the graph currently supports.
// ---------------------------------------------------------------------------

/// A single conversation turn (user or agent).
#[derive(Debug, Clone)]
pub struct Turn {
    pub role: TurnRole,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnRole {
    User,
    Agent,
}

/// The clinical intake phases. Not strictly linear — Differentiation loops
/// back on itself as long as high-value differentiators remain AND the user
/// is engaged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntakePhase {
    /// "What brings you in today?"
    ChiefComplaint,
    /// OLDCARTS deep-dive on each CC
    Hpi,
    /// Graph-guided system sweep
    ReviewOfSystems,
    /// Narrowing candidates via discriminating questions
    Differentiation,
    /// Check if user's symptoms are adverse reactions to disclosed supplements
    CausationInquiry,
    /// Final results presentation
    Recommendation,
}

/// OLDCARTS mnemonic state — tracks what clinical dimensions have been gathered.
#[derive(Debug, Clone, Default)]
pub struct OldcartsState {
    pub onset: Option<String>,
    pub location: Option<String>,
    pub duration: Option<String>,
    pub character: Option<String>,
    pub aggravating: Vec<String>,
    pub alleviating: Vec<String>,
    pub radiation: Option<String>,
    pub timing: Option<String>,
    pub severity: Option<u8>, // 1-10
}

impl OldcartsState {
    /// Returns OLDCARTS dimensions that haven't been gathered yet.
    pub fn gaps(&self) -> Vec<&'static str> {
        let mut gaps = Vec::new();
        if self.onset.is_none() {
            gaps.push("Onset");
        }
        if self.location.is_none() {
            gaps.push("Location");
        }
        if self.duration.is_none() {
            gaps.push("Duration");
        }
        if self.character.is_none() {
            gaps.push("Character");
        }
        if self.aggravating.is_empty() {
            gaps.push("Aggravating factors");
        }
        if self.alleviating.is_empty() {
            gaps.push("Alleviating factors");
        }
        if self.radiation.is_none() {
            gaps.push("Radiation");
        }
        if self.timing.is_none() {
            gaps.push("Timing");
        }
        if self.severity.is_none() {
            gaps.push("Severity");
        }
        gaps
    }

    /// How many of the 9 dimensions have been filled.
    pub fn filled_count(&self) -> usize {
        9 - self.gaps().len()
    }

    /// Returns the set of OLDCARTS dimensions that have been filled.
    pub fn filled_dimensions(&self) -> HashSet<OldcartsDimension> {
        let mut filled = HashSet::new();
        if self.onset.is_some() { filled.insert(OldcartsDimension::Onset); }
        if self.location.is_some() { filled.insert(OldcartsDimension::Location); }
        if self.duration.is_some() { filled.insert(OldcartsDimension::Duration); }
        if self.character.is_some() { filled.insert(OldcartsDimension::Character); }
        if !self.aggravating.is_empty() { filled.insert(OldcartsDimension::Aggravating); }
        if !self.alleviating.is_empty() { filled.insert(OldcartsDimension::Alleviating); }
        if self.radiation.is_some() { filled.insert(OldcartsDimension::Radiation); }
        if self.timing.is_some() { filled.insert(OldcartsDimension::Timing); }
        if self.severity.is_some() { filled.insert(OldcartsDimension::Severity); }
        filled
    }
}

/// A chief complaint with mapped graph concepts.
#[derive(Debug, Clone)]
pub struct ChiefComplaint {
    /// The user's exact words
    pub raw_text: String,
    /// Graph Symptom node names this maps to
    pub mapped_symptoms: Vec<String>,
    /// Graph System node names this maps to
    pub mapped_systems: Vec<String>,
    /// Accompanying symptoms discovered during HPI
    pub associated_symptoms: Vec<String>,
}

impl ChiefComplaint {
    pub fn new(raw_text: impl Into<String>) -> Self {
        Self {
            raw_text: raw_text.into(),
            mapped_symptoms: Vec::new(),
            mapped_systems: Vec::new(),
            associated_symptoms: Vec::new(),
        }
    }
}

/// Required safety touchpoints that must be completed before a recommendation
/// can be unlocked. Every flag is set only when the bot explicitly asks the
/// corresponding question — never from user-volunteered information alone.
/// Partial ordering: contraindications_checked requires all three preceding
/// flags to be true first.
#[derive(Debug, Clone, Default)]
pub struct IntakeChecklist {
    /// Bot explicitly asked about current prescription medications.
    pub prescriptions_asked: bool,
    /// Bot explicitly asked about OTC medications and other supplements.
    pub otc_and_supplements_asked: bool,
    /// Bot explicitly asked about relevant health conditions (pregnancy, kidney
    /// disease, etc.).
    pub health_conditions_asked: bool,
    /// Contraindication check has been run against all disclosed inputs.
    /// Only valid after the three prerequisites above are true.
    pub contraindications_checked: bool,
}

impl IntakeChecklist {
    /// True when all prerequisites for a recommendation are satisfied.
    pub fn complete(&self) -> bool {
        self.prescriptions_asked
            && self.otc_and_supplements_asked
            && self.health_conditions_asked
            && self.contraindications_checked
    }

    /// True when all three prerequisite questions have been asked,
    /// meaning the contraindication check is now allowed to run.
    pub fn contraindications_ready(&self) -> bool {
        self.prescriptions_asked
            && self.otc_and_supplements_asked
            && self.health_conditions_asked
    }

    /// Returns the template ID of the next unchecked required question,
    /// or None if complete (contraindication check is handled separately).
    pub fn next_required_question(&self) -> Option<&'static str> {
        if !self.prescriptions_asked {
            return Some("ask_prescriptions");
        }
        if !self.otc_and_supplements_asked {
            return Some("ask_otc_supplements");
        }
        if !self.health_conditions_asked {
            return Some("ask_health_conditions");
        }
        None
    }
}

/// The full state of one intake conversation.
#[derive(Debug)]
pub struct IntakeSession {
    pub id: Uuid,
    pub phase: IntakePhase,
    pub chief_complaints: Vec<ChiefComplaint>,
    pub oldcarts: OldcartsState,
    pub systems_reviewed: HashSet<String>,
    /// Systems explicitly denied by user (pertinent negatives)
    pub systems_denied: HashSet<String>,
    pub candidates: CandidateSet,
    /// Complexity lens level — escalates as detail accumulates
    pub lens_level: f64,
    /// Full conversation history
    pub turns: Vec<Turn>,
    /// Compressed history after ~8 turns
    pub turn_summary: Option<String>,
    /// Disclosed conditions/medications (for contraindication filtering)
    pub contraindications: Vec<String>,
    /// Safety checklist — tracks required clinical touchpoints.
    pub checklist: IntakeChecklist,
    /// Intake KG: question template IDs already asked this session.
    pub visited_questions: HashSet<String>,
    /// Intake KG: how many times each goal has been probed.
    pub goal_ask_counts: HashMap<String, u8>,
    /// Intake KG: SymptomProfile IDs from concept mapping.
    pub active_profiles: Vec<String>,
    /// Supplements the user disclosed they're currently taking.
    pub disclosed_supplements: Vec<String>,
    /// Differentiator count from last turn's executor results.
    pub last_differentiator_count: usize,
    /// How many turns have been spent in the Differentiation phase.
    pub differentiation_turns: usize,
}

impl IntakeSession {
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            phase: IntakePhase::ChiefComplaint,
            chief_complaints: Vec::new(),
            oldcarts: OldcartsState::default(),
            systems_reviewed: HashSet::new(),
            systems_denied: HashSet::new(),
            candidates: CandidateSet::new(),
            lens_level: 0.15, // start at 5th-grade level
            turns: Vec::new(),
            turn_summary: None,
            contraindications: Vec::new(),
            checklist: IntakeChecklist::default(),
            visited_questions: HashSet::new(),
            goal_ask_counts: HashMap::new(),
            active_profiles: Vec::new(),
            disclosed_supplements: Vec::new(),
            last_differentiator_count: 0,
            differentiation_turns: 0,
        }
    }

    /// Record a user message.
    pub fn add_user_turn(&mut self, text: impl Into<String>) {
        self.turns.push(Turn {
            role: TurnRole::User,
            text: text.into(),
        });
    }

    /// Record an agent response.
    pub fn add_agent_turn(&mut self, text: impl Into<String>) {
        self.turns.push(Turn {
            role: TurnRole::Agent,
            text: text.into(),
        });
    }

    /// Number of conversation turns.
    pub fn turn_count(&self) -> usize {
        self.turns.len()
    }

    /// Whether the conversation history should be compressed.
    pub fn needs_compression(&self) -> bool {
        self.turn_summary.is_none() && self.turns.len() > 16
    }

    /// Add a chief complaint from raw user text.
    pub fn add_complaint(&mut self, raw_text: impl Into<String>) {
        self.chief_complaints.push(ChiefComplaint::new(raw_text));
    }

    /// Revise a chief complaint — remove old mapping and prepare for re-mapping.
    /// Returns true if the complaint was found and revised.
    pub fn revise_complaint(&mut self, old_text: &str, new_text: &str) -> bool {
        let old_lower = old_text.to_lowercase();
        if let Some(cc) = self
            .chief_complaints
            .iter_mut()
            .find(|c| c.raw_text.to_lowercase() == old_lower)
        {
            cc.raw_text = new_text.to_string();
            cc.mapped_symptoms.clear();
            cc.mapped_systems.clear();
            cc.associated_symptoms.clear();
            true
        } else {
            false
        }
    }

    /// Record a system as explicitly denied (pertinent negative).
    pub fn deny_system(&mut self, system: impl Into<String>) {
        let s = system.into();
        self.systems_denied.insert(s.clone());
        self.systems_reviewed.insert(s);
    }

    /// Record a system as reviewed (not denied).
    pub fn review_system(&mut self, system: impl Into<String>) {
        self.systems_reviewed.insert(system.into());
    }

    /// Escalate the lens level, clamped to [0.0, 1.0].
    pub fn escalate_lens(&mut self, new_level: f64) {
        self.lens_level = new_level.clamp(0.0, 1.0).max(self.lens_level);
    }

    /// All symptom names across all chief complaints (including associated).
    pub fn all_symptoms(&self) -> Vec<&str> {
        self.chief_complaints
            .iter()
            .flat_map(|cc| {
                cc.mapped_symptoms
                    .iter()
                    .chain(cc.associated_symptoms.iter())
                    .map(|s| s.as_str())
            })
            .collect()
    }

    /// All system names across all chief complaints.
    pub fn all_systems(&self) -> Vec<&str> {
        self.chief_complaints
            .iter()
            .flat_map(|cc| cc.mapped_systems.iter().map(|s| s.as_str()))
            .collect()
    }
}

impl Default for IntakeSession {
    fn default() -> Self {
        Self::new()
    }
}
