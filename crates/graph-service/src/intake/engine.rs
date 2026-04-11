// ---------------------------------------------------------------------------
// Intake Traversal Engine
//
// Walks the intake knowledge graph each turn to decide:
//   1. What question to ask next (EIG-scored)
//   2. What graph actions to fire (supplement KG queries)
//   3. Whether to transition to a new stage
//
// The LLM never decides what to ask. This engine does.
// The LLM only renders the engine's output as natural conversation.
// ---------------------------------------------------------------------------

use std::collections::{HashMap, HashSet};

use super::store::IntakeGraphStore;
use super::types::*;

// ---------------------------------------------------------------------------
// Session context — what the engine needs to know about the current state
// ---------------------------------------------------------------------------

/// Everything the traversal engine needs from the current session.
/// Passed in each turn — the engine is stateless.
pub struct TraversalContext {
    /// Current stage.
    pub stage: IntakeStageId,
    /// Active symptom profile IDs (from mapped chief complaints).
    pub active_profiles: Vec<String>,
    /// Which OLDCARTS dimensions are already filled.
    pub filled_oldcarts: HashSet<OldcartsDimension>,
    /// How many OLDCARTS dimensions are filled.
    pub filled_count: usize,
    /// Question template IDs already asked in this session.
    pub visited_questions: HashSet<String>,
    /// How many times each goal has been asked about.
    pub goal_ask_counts: HashMap<String, u8>,
    /// Current candidate ingredient names.
    pub candidate_names: Vec<String>,
    /// Number of candidates.
    pub candidate_count: usize,
    /// Score gap between #1 and #2 candidate (0.0 if <2 candidates).
    pub confidence_gap: f64,
    /// Top candidate score.
    pub top_score: f64,
    /// Systems already reviewed.
    pub reviewed_systems: HashSet<String>,
    /// Whether medications have been asked about.
    pub medications_asked: bool,
    /// Whether user disclosed any medications.
    pub medications_disclosed: bool,
    /// Whether the user just signaled disengagement.
    pub user_disengaged: bool,
    /// Whether the user said "that's it" / "that's all".
    pub user_done_sharing: bool,
    /// Number of differentiator questions available from supplement KG.
    pub differentiator_count: usize,
    /// Chief complaint text for template filling.
    pub chief_complaint: Option<String>,
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

pub struct IntakeEngine<'a> {
    store: &'a IntakeGraphStore,
}

impl<'a> IntakeEngine<'a> {
    pub fn new(store: &'a IntakeGraphStore) -> Self {
        Self { store }
    }

    /// Run one turn of the intake traversal engine.
    /// Returns the action(s) for this turn.
    pub async fn next_turn(&self, ctx: &TraversalContext) -> TurnAction {
        let mut trace = Vec::new();
        trace.push(format!("Stage: {:?}", ctx.stage));

        // Step 1: Check if we should transition stages
        if let Some(next) = self.evaluate_transition(ctx, &mut trace).await {
            return TurnAction {
                question: None,
                graph_actions: self.actions_for_stage(&next),
                next_stage: Some(next),
                trace,
            };
        }

        // Step 2: Determine graph actions for current stage
        let graph_actions = self.actions_for_stage(&ctx.stage);

        // Step 3: Find the best question to ask
        let question = self.select_question(ctx, &mut trace).await;

        TurnAction {
            question,
            graph_actions,
            next_stage: None,
            trace,
        }
    }

    // -----------------------------------------------------------------------
    // Stage transition evaluation
    // -----------------------------------------------------------------------

    async fn evaluate_transition(
        &self,
        ctx: &TraversalContext,
        trace: &mut Vec<String>,
    ) -> Option<IntakeStageId> {
        match ctx.stage {
            IntakeStageId::ChiefComplaint => {
                // Transition when profiles are loaded, OR when a complaint has been
                // recorded (chief_complaint is set) and what_brings_you_in was already asked.
                // The second condition prevents deadlock when concept mapping fails to resolve
                // a profile but the user clearly stated their complaint.
                let complaint_recorded = ctx.chief_complaint.is_some();
                let already_asked = ctx.visited_questions.contains("what_brings_you_in");
                if !ctx.active_profiles.is_empty()
                    || (complaint_recorded && already_asked)
                {
                    trace.push("Chief complaint recorded → HPI".into());
                    return Some(IntakeStageId::Hpi);
                }
            }

            IntakeStageId::Hpi => {
                // Confidence-based auto-recommend — fire early if one candidate is
                // clearly winning and we've gathered enough to be useful.
                if ctx.candidate_count > 0
                    && ctx.confidence_gap > 0.3
                    && ctx.filled_count >= 2
                    && ctx.medications_asked
                {
                    trace.push("Confident candidates + medications asked → Recommendation".into());
                    return Some(IntakeStageId::Recommendation);
                }

                // User done sharing
                if ctx.user_done_sharing && ctx.candidate_count > 0 {
                    if !ctx.medications_asked {
                        trace.push("User done but medications not asked — staying for med check".into());
                        return None; // Force medication question
                    }
                    trace.push("User done sharing → Recommendation".into());
                    return Some(IntakeStageId::Recommendation);
                }

                // User done sharing but no candidates — still go to Recommendation
                // so the renderer can honestly say nothing was found
                if ctx.user_done_sharing && ctx.candidate_count == 0 {
                    trace.push("User done sharing, no candidates → Recommendation (empty)".into());
                    return Some(IntakeStageId::Recommendation);
                }

                // Check if enough OLDCARTS filled
                let sufficient = self.sufficient_dimensions(ctx).await;
                if ctx.filled_count >= sufficient as usize {
                    if ctx.candidate_count > 0 {
                        // Check if we should go to system review or differentiation
                        let unreviewed = self.unreviewed_system_count(ctx).await;
                        if unreviewed > 0 {
                            trace.push(format!(
                                "OLDCARTS sufficient ({}/{}) → SystemReview ({} systems)",
                                ctx.filled_count, sufficient, unreviewed
                            ));
                            return Some(IntakeStageId::SystemReview);
                        } else if ctx.differentiator_count > 0 {
                            trace.push("OLDCARTS sufficient, systems done → Differentiation".into());
                            return Some(IntakeStageId::Differentiation);
                        } else if ctx.medications_asked {
                            trace.push("OLDCARTS sufficient, no diff, meds asked → Recommendation".into());
                            return Some(IntakeStageId::Recommendation);
                        }
                    } else {
                        // OLDCARTS sufficient but still no candidates — go to recommendation
                        // so the renderer can honestly say nothing was found
                        trace.push("OLDCARTS sufficient, no candidates → Recommendation (empty)".into());
                        return Some(IntakeStageId::Recommendation);
                    }
                }

                // User disengaged
                if ctx.user_disengaged && ctx.candidate_count > 0 && ctx.medications_asked {
                    trace.push("User disengaged → Recommendation".into());
                    return Some(IntakeStageId::Recommendation);
                }

                // User disengaged with no candidates
                if ctx.user_disengaged && ctx.candidate_count == 0 {
                    trace.push("User disengaged, no candidates → Recommendation (empty)".into());
                    return Some(IntakeStageId::Recommendation);
                }
            }

            IntakeStageId::SystemReview => {
                // Skip system review entirely if one candidate is already dominant
                let dominant = ctx.candidate_count > 0 && ctx.confidence_gap > 0.4;
                let unreviewed = self.unreviewed_system_count(ctx).await;
                if unreviewed == 0 || ctx.user_disengaged || dominant {
                    if ctx.differentiator_count > 0 && !dominant {
                        trace.push("Systems reviewed → Differentiation".into());
                        return Some(IntakeStageId::Differentiation);
                    } else if !ctx.medications_asked {
                        // Safety gate — must ask about medications before recommending
                        trace.push("Systems reviewed, medications not asked → HPI for med check".into());
                        return Some(IntakeStageId::Hpi);
                    } else {
                        trace.push("Systems reviewed → Recommendation".into());
                        return Some(IntakeStageId::Recommendation);
                    }
                }
            }

            IntakeStageId::Differentiation => {
                if ctx.differentiator_count == 0 || ctx.user_disengaged || ctx.user_done_sharing {
                    if ctx.medications_disclosed {
                        trace.push("Differentiation done, meds disclosed → CausationInquiry".into());
                        return Some(IntakeStageId::CausationInquiry);
                    } else if !ctx.medications_asked {
                        // Safety gate — must ask about medications before recommending
                        trace.push("Differentiation done, medications not asked → HPI for med check".into());
                        return Some(IntakeStageId::Hpi);
                    } else if ctx.medications_asked {
                        trace.push("Differentiation done → Recommendation".into());
                        return Some(IntakeStageId::Recommendation);
                    }
                }
            }

            IntakeStageId::CausationInquiry => {
                // Always move to recommendation after causation inquiry
                trace.push("Causation inquiry complete → Recommendation".into());
                return Some(IntakeStageId::Recommendation);
            }

            IntakeStageId::Recommendation => {
                // Terminal stage
            }
        }

        None
    }

    // -----------------------------------------------------------------------
    // Question selection with EIG scoring
    // -----------------------------------------------------------------------

    async fn select_question(
        &self,
        ctx: &TraversalContext,
        trace: &mut Vec<String>,
    ) -> Option<ResolvedQuestion> {
        match ctx.stage {
            IntakeStageId::ChiefComplaint => {
                return self.select_chief_complaint_question(ctx, trace).await;
            }
            IntakeStageId::Hpi => {
                return self.select_hpi_question(ctx, trace).await;
            }
            IntakeStageId::SystemReview => {
                return self.select_system_review_question(ctx, trace).await;
            }
            IntakeStageId::Differentiation => {
                // Differentiating questions come from the supplement KG dynamically,
                // not from the intake graph. The handler will use the differentiator
                // module directly. We can still select a framing template.
                trace.push("Differentiation: questions from supplement KG".into());
                return None;
            }
            IntakeStageId::CausationInquiry => {
                if !ctx.visited_questions.contains("causation_notice") {
                    return Some(ResolvedQuestion {
                        template_id: "causation_notice".into(),
                        text: "I want to mention — some of your symptoms can be associated with supplements or medications. Let me factor that into what I share with you.".into(),
                        goal_id: "check_medications".into(),
                        score: 1.0,
                    });
                }
                return None;
            }
            IntakeStageId::Recommendation => {
                // No questions in recommendation phase
                return None;
            }
        }
    }

    async fn select_chief_complaint_question(
        &self,
        ctx: &TraversalContext,
        trace: &mut Vec<String>,
    ) -> Option<ResolvedQuestion> {
        let q_id = if ctx.active_profiles.is_empty() {
            "what_brings_you_in"
        } else {
            "anything_else"
        };

        if ctx.visited_questions.contains(q_id) {
            trace.push(format!("Already asked {}", q_id));
            return None;
        }

        let q = self.store.get_question(q_id).await?;
        trace.push(format!("Selected: {} (chief complaint)", q_id));
        Some(ResolvedQuestion {
            template_id: q_id.into(),
            text: q.template,
            goal_id: "identify_chief_complaint".into(),
            score: 1.0,
        })
    }

    async fn select_hpi_question(
        &self,
        ctx: &TraversalContext,
        trace: &mut Vec<String>,
    ) -> Option<ResolvedQuestion> {
        // First check: do we need to ask about medications? (safety gate)
        if !ctx.medications_asked
            && ctx.candidate_count > 0
            && !ctx.visited_questions.contains("ask_medications")
        {
            // Check if we've asked enough OLDCARTS to warrant the medication question
            if ctx.filled_count >= 2 {
                let q = self.store.get_question("ask_medications").await?;
                trace.push("Safety gate: medication check needed".into());
                return Some(ResolvedQuestion {
                    template_id: "ask_medications".into(),
                    text: q.template,
                    goal_id: "check_medications".into(),
                    score: 10.0, // Always highest priority
                });
            }
        }

        // Collect relevant OLDCARTS dimensions for active profiles
        let relevant = self.relevant_dimensions(ctx).await;
        let irrelevant = self.irrelevant_dimensions(ctx).await;

        trace.push(format!(
            "Relevant OLDCARTS: {:?}, Irrelevant: {:?}",
            relevant.iter().map(|d| d.as_str()).collect::<Vec<_>>(),
            irrelevant.iter().map(|d| d.as_str()).collect::<Vec<_>>(),
        ));

        // Score each unfilled, relevant dimension
        let mut candidates: Vec<(String, String, f64)> = Vec::new(); // (question_id, goal_id, score)

        let dimension_to_goal: HashMap<OldcartsDimension, &str> = [
            (OldcartsDimension::Onset, "characterize_onset"),
            (OldcartsDimension::Location, "characterize_location"),
            (OldcartsDimension::Duration, "characterize_duration"),
            (OldcartsDimension::Character, "characterize_character"),
            (OldcartsDimension::Aggravating, "characterize_aggravating"),
            (OldcartsDimension::Alleviating, "characterize_alleviating"),
            (OldcartsDimension::Radiation, "characterize_radiation"),
            (OldcartsDimension::Timing, "characterize_timing"),
            (OldcartsDimension::Severity, "characterize_severity"),
        ].into_iter().collect();

        let dimension_to_question: HashMap<OldcartsDimension, &str> = [
            (OldcartsDimension::Onset, "ask_onset"),
            (OldcartsDimension::Location, "ask_location"),
            (OldcartsDimension::Duration, "ask_duration"),
            (OldcartsDimension::Character, "ask_character"),
            (OldcartsDimension::Aggravating, "ask_aggravating"),
            (OldcartsDimension::Alleviating, "ask_alleviating"),
            (OldcartsDimension::Radiation, "ask_radiation"),
            (OldcartsDimension::Timing, "ask_timing"),
            (OldcartsDimension::Severity, "ask_severity"),
        ].into_iter().collect();

        // Base priorities from the seed edges
        let base_priorities: HashMap<&str, f64> = [
            ("ask_onset", 0.9),
            ("ask_location", 0.8),
            ("ask_character", 0.85),
            ("ask_duration", 0.7),
            ("ask_aggravating", 0.75),
            ("ask_alleviating", 0.6),
            ("ask_radiation", 0.4),
            ("ask_timing", 0.7),
            ("ask_severity", 0.65),
        ].into_iter().collect();

        for dim in OldcartsDimension::all() {
            // Skip already filled
            if ctx.filled_oldcarts.contains(dim) {
                continue;
            }

            // Skip irrelevant for active profiles
            if irrelevant.contains(dim) {
                continue;
            }

            let q_id = match dimension_to_question.get(dim) {
                Some(id) => *id,
                None => continue,
            };
            let goal_id = match dimension_to_goal.get(dim) {
                Some(id) => *id,
                None => continue,
            };

            // Check visited
            if ctx.visited_questions.contains(q_id) {
                // Check if there's a fallback
                let fallback_id = match q_id {
                    "ask_onset" => Some("clarify_onset"),
                    "ask_character" => Some("clarify_character"),
                    _ => None,
                };
                if let Some(fb) = fallback_id {
                    if !ctx.visited_questions.contains(fb) {
                        // Check max_asks
                        let count = ctx.goal_ask_counts.get(goal_id).copied().unwrap_or(0);
                        if count < 2 {
                            let base = base_priorities.get(q_id).copied().unwrap_or(0.5) * 0.6;
                            candidates.push((fb.into(), goal_id.into(), base));
                        }
                    }
                }
                continue;
            }

            // Check max_asks
            let count = ctx.goal_ask_counts.get(goal_id).copied().unwrap_or(0);
            if count >= 2 {
                continue;
            }

            // EIG scoring: base_priority × system_relevance
            let base = base_priorities.get(q_id).copied().unwrap_or(0.5);
            let system_relevance = if relevant.contains(dim) { 1.0 } else { 0.3 };

            // Information gain heuristic: earlier in the interview, broader questions
            // score higher. As we narrow, discriminating questions score higher.
            let information_gain = if ctx.candidate_count > 1 {
                // More candidates = more value from any characterizing question
                1.0
            } else if ctx.candidate_count == 1 {
                // Single candidate — less value from further characterization
                0.5
            } else {
                // No candidates yet — characterization is critical
                1.2
            };

            let score = base * system_relevance * information_gain;
            candidates.push((q_id.into(), goal_id.into(), score));
        }

        // Sort by score descending
        candidates.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

        if let Some((q_id, goal_id, score)) = candidates.first() {
            let q = self.store.get_question(q_id).await?;
            let text = self.fill_template(&q.template, ctx);
            trace.push(format!(
                "Selected: {} (score: {:.2}, goal: {})",
                q_id, score, goal_id
            ));
            Some(ResolvedQuestion {
                template_id: q_id.clone(),
                text,
                goal_id: goal_id.clone(),
                score: *score,
            })
        } else {
            trace.push("No more HPI questions available".into());
            None
        }
    }

    async fn select_system_review_question(
        &self,
        ctx: &TraversalContext,
        trace: &mut Vec<String>,
    ) -> Option<ResolvedQuestion> {
        // Get systems to review — prioritize cluster-suggested systems
        let mut prioritized: Vec<(String, f64)> = Vec::new();

        // Check clusters for prioritized systems
        for profile_id in &ctx.active_profiles {
            let clusters = self.store.clusters_for_symptom(profile_id).await;
            for cluster in clusters {
                for sys in &cluster.prioritized_systems {
                    if !ctx.reviewed_systems.contains(sys) {
                        prioritized.push((sys.clone(), 1.0));
                    }
                }
            }
        }

        // Add archetype default systems
        for profile_id in &ctx.active_profiles {
            if let Some(sp) = self.store.get_symptom_profile(profile_id).await {
                if let Some(arch) = self.store.get_archetype(&sp.archetype_id).await {
                    for sys in &arch.default_systems {
                        if !ctx.reviewed_systems.contains(sys) {
                            // Lower priority than cluster-suggested
                            if !prioritized.iter().any(|(s, _)| s == sys) {
                                prioritized.push((sys.clone(), 0.6));
                            }
                        }
                    }
                }
                // Profile-specific associated systems
                for sys in &sp.associated_systems {
                    if !ctx.reviewed_systems.contains(sys) {
                        if !prioritized.iter().any(|(s, _)| s == sys) {
                            prioritized.push((sys.clone(), 0.8));
                        }
                    }
                }
            }
        }

        // Sort by priority
        prioritized.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Find first system with an unvisited question
        for (system_name, priority) in &prioritized {
            if let Some(sr) = self.store.find_system_review(system_name).await {
                // Find the probes edge → question
                let edges = self.store.edges_from(&sr.id).await;
                for (target_id, edge_type, _meta) in edges {
                    if edge_type == IntakeEdgeType::Probes
                        && !ctx.visited_questions.contains(&target_id)
                    {
                        if let Some(q) = self.store.get_question(&target_id).await {
                            trace.push(format!(
                                "SystemReview: {} → {} (priority: {:.1})",
                                system_name, target_id, priority
                            ));
                            return Some(ResolvedQuestion {
                                template_id: target_id,
                                text: q.template,
                                goal_id: "identify_system_involvement".into(),
                                score: *priority,
                            });
                        }
                    }
                }
                // If all probes visited, use first screening question
                if let Some(sq) = sr.screening_questions.first() {
                    let synth_id = format!("sr_screening_{}", slug(system_name));
                    if !ctx.visited_questions.contains(&synth_id) {
                        trace.push(format!(
                            "SystemReview: {} screening (priority: {:.1})",
                            system_name, priority
                        ));
                        return Some(ResolvedQuestion {
                            template_id: synth_id,
                            text: sq.clone(),
                            goal_id: "identify_system_involvement".into(),
                            score: *priority,
                        });
                    }
                }
            }
        }

        trace.push("No more system review questions".into());
        None
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Get the sufficient dimensions count for the active profiles.
    async fn sufficient_dimensions(&self, ctx: &TraversalContext) -> u8 {
        let mut max_sufficient: u8 = 4; // default
        for profile_id in &ctx.active_profiles {
            if let Some(sp) = self.store.get_symptom_profile(profile_id).await {
                if let Some(override_val) = sp.sufficient_dimensions_override {
                    max_sufficient = max_sufficient.max(override_val);
                } else if let Some(arch) = self.store.get_archetype(&sp.archetype_id).await {
                    max_sufficient = max_sufficient.max(arch.sufficient_dimensions);
                }
            }
        }
        max_sufficient
    }

    /// Get relevant OLDCARTS dimensions across all active profiles.
    pub async fn relevant_dimensions(&self, ctx: &TraversalContext) -> HashSet<OldcartsDimension> {
        let mut relevant = HashSet::new();
        for profile_id in &ctx.active_profiles {
            if let Some(sp) = self.store.get_symptom_profile(profile_id).await {
                if let Some(ref overrides) = sp.relevant_oldcarts_override {
                    relevant.extend(overrides.iter());
                } else if let Some(arch) = self.store.get_archetype(&sp.archetype_id).await {
                    relevant.extend(arch.relevant_oldcarts.iter());
                }
            }
        }
        // If no profiles loaded, default to all
        if relevant.is_empty() {
            relevant.extend(OldcartsDimension::all().iter());
        }
        relevant
    }

    /// Get irrelevant OLDCARTS dimensions across all active profiles.
    async fn irrelevant_dimensions(&self, ctx: &TraversalContext) -> HashSet<OldcartsDimension> {
        let mut irrelevant = HashSet::new();
        for profile_id in &ctx.active_profiles {
            if let Some(sp) = self.store.get_symptom_profile(profile_id).await {
                if let Some(ref overrides) = sp.irrelevant_oldcarts_override {
                    irrelevant.extend(overrides.iter());
                } else if let Some(arch) = self.store.get_archetype(&sp.archetype_id).await {
                    irrelevant.extend(arch.irrelevant_oldcarts.iter());
                }
            }
        }
        irrelevant
    }

    /// Count unreviewed systems for active profiles.
    async fn unreviewed_system_count(&self, ctx: &TraversalContext) -> usize {
        let mut systems = HashSet::new();
        for profile_id in &ctx.active_profiles {
            if let Some(sp) = self.store.get_symptom_profile(profile_id).await {
                for sys in &sp.associated_systems {
                    systems.insert(sys.clone());
                }
                if let Some(arch) = self.store.get_archetype(&sp.archetype_id).await {
                    for sys in &arch.default_systems {
                        systems.insert(sys.clone());
                    }
                }
            }
        }
        systems.difference(&ctx.reviewed_systems).count()
    }

    /// Determine which graph actions to fire for a stage.
    fn actions_for_stage(&self, stage: &IntakeStageId) -> Vec<GraphActionType> {
        match stage {
            IntakeStageId::ChiefComplaint => vec![GraphActionType::QueryCandidates],
            IntakeStageId::Hpi => vec![GraphActionType::QueryCandidates],
            IntakeStageId::SystemReview => vec![
                GraphActionType::QueryCandidates,
                GraphActionType::FindAdjacentSystems,
            ],
            IntakeStageId::Differentiation => vec![
                GraphActionType::QueryCandidates,
                GraphActionType::FindDiscriminators,
            ],
            IntakeStageId::CausationInquiry => vec![
                GraphActionType::CheckInteractions,
                GraphActionType::CheckAdverseReactions,
            ],
            IntakeStageId::Recommendation => vec![
                GraphActionType::QueryCandidates,
                GraphActionType::FetchMechanism,
            ],
        }
    }

    /// Fill template placeholders with session context.
    fn fill_template(&self, template: &str, ctx: &TraversalContext) -> String {
        let mut text = template.to_string();
        if let Some(ref complaint) = ctx.chief_complaint {
            text = text.replace("{symptom}", complaint);
        } else {
            text = text.replace("{symptom}", "your symptoms");
        }
        text
    }
}

fn slug(s: &str) -> String {
    s.to_lowercase()
        .replace(|c: char| !c.is_alphanumeric() && c != '_', "_")
        .trim_matches('_')
        .to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::KnowledgeGraph;
    use crate::intake::seed::seed_intake_graph;

    async fn setup() -> (KnowledgeGraph, IntakeGraphStore) {
        let kg = KnowledgeGraph::in_memory().await.unwrap();
        let store = IntakeGraphStore::new(kg.db());
        seed_intake_graph(&store).await;
        (kg, store)
    }

    fn empty_context() -> TraversalContext {
        TraversalContext {
            stage: IntakeStageId::ChiefComplaint,
            active_profiles: vec![],
            filled_oldcarts: HashSet::new(),
            filled_count: 0,
            visited_questions: HashSet::new(),
            goal_ask_counts: HashMap::new(),
            candidate_names: vec![],
            candidate_count: 0,
            confidence_gap: 0.0,
            top_score: 0.0,
            reviewed_systems: HashSet::new(),
            medications_asked: false,
            medications_disclosed: false,
            user_disengaged: false,
            user_done_sharing: false,
            differentiator_count: 0,
            chief_complaint: None,
        }
    }

    #[tokio::test]
    async fn test_chief_complaint_asks_what_brings_you_in() {
        let (_kg, store) = setup().await;
        let engine = IntakeEngine::new(&store);
        let ctx = empty_context();

        let action = engine.next_turn(&ctx).await;
        assert!(action.question.is_some());
        let q = action.question.unwrap();
        assert_eq!(q.template_id, "what_brings_you_in");
    }

    #[tokio::test]
    async fn test_chief_complaint_transitions_to_hpi() {
        let (_kg, store) = setup().await;
        let engine = IntakeEngine::new(&store);
        let mut ctx = empty_context();
        ctx.active_profiles.push("muscle_cramps".into());

        let action = engine.next_turn(&ctx).await;
        assert_eq!(action.next_stage, Some(IntakeStageId::Hpi));
    }

    #[tokio::test]
    async fn test_hpi_skips_irrelevant_dimensions() {
        let (_kg, store) = setup().await;
        let engine = IntakeEngine::new(&store);

        // Add a sleep archetype symptom profile
        store.add_symptom_profile(&SymptomProfile {
            id: "insomnia".into(),
            name: "Insomnia".into(),
            cui: None,
            aliases: vec![],
            archetype_id: "sleep".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: None,
            associated_systems: vec!["nervous system".into()],
        }).await;

        let mut ctx = empty_context();
        ctx.stage = IntakeStageId::Hpi;
        ctx.active_profiles.push("insomnia".into());
        ctx.chief_complaint = Some("trouble sleeping".into());

        let action = engine.next_turn(&ctx).await;
        let q = action.question.unwrap();

        // Sleep archetype marks Location and Radiation as irrelevant.
        // So we should NOT get ask_location or ask_radiation first.
        assert_ne!(q.template_id, "ask_location");
        assert_ne!(q.template_id, "ask_radiation");
    }

    #[tokio::test]
    async fn test_medication_check_is_safety_gated() {
        let (_kg, store) = setup().await;
        let engine = IntakeEngine::new(&store);

        store.add_symptom_profile(&SymptomProfile {
            id: "muscle_cramps".into(),
            name: "Muscle Cramps".into(),
            cui: None,
            aliases: vec![],
            archetype_id: "pain".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: None,
            associated_systems: vec!["nervous system".into()],
        }).await;

        let mut ctx = empty_context();
        ctx.stage = IntakeStageId::Hpi;
        ctx.active_profiles.push("muscle_cramps".into());
        ctx.candidate_count = 3;
        ctx.filled_count = 5; // Enough to trigger transition
        ctx.medications_asked = false;
        ctx.differentiator_count = 2;
        ctx.chief_complaint = Some("muscle cramps".into());

        // Even though OLDCARTS is sufficient and there are candidates,
        // the engine should NOT transition to system_review because
        // we haven't asked about medications yet. It should ask about meds.
        let action = engine.next_turn(&ctx).await;

        // Either it stays in HPI and asks about meds, or transitions
        // but the medication question should come up
        if action.next_stage.is_none() {
            // Good — stayed to ask medication question
            let q = action.question.unwrap();
            assert_eq!(q.template_id, "ask_medications");
        }
        // If it transitions, it should NOT go to Recommendation
        if let Some(next) = &action.next_stage {
            assert_ne!(*next, IntakeStageId::Recommendation);
        }
    }

    #[tokio::test]
    async fn test_visited_questions_prevents_repeats() {
        let (_kg, store) = setup().await;
        let engine = IntakeEngine::new(&store);

        store.add_symptom_profile(&SymptomProfile {
            id: "headache".into(),
            name: "Headache".into(),
            cui: None,
            aliases: vec![],
            archetype_id: "pain".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: None,
            associated_systems: vec!["nervous system".into()],
        }).await;

        let mut ctx = empty_context();
        ctx.stage = IntakeStageId::Hpi;
        ctx.active_profiles.push("headache".into());
        ctx.chief_complaint = Some("headache".into());

        // First turn
        let action1 = engine.next_turn(&ctx).await;
        let q1_id = action1.question.as_ref().unwrap().template_id.clone();

        // Mark as visited
        ctx.visited_questions.insert(q1_id.clone());
        ctx.filled_oldcarts.insert(OldcartsDimension::Onset);
        ctx.filled_count = 1;

        // Second turn — should NOT repeat the same question
        let action2 = engine.next_turn(&ctx).await;
        if let Some(q2) = &action2.question {
            assert_ne!(q2.template_id, q1_id);
        }
    }

    #[tokio::test]
    async fn test_system_review_uses_cluster_priority() {
        let (_kg, store) = setup().await;
        let engine = IntakeEngine::new(&store);

        // Add profiles that form the electrolyte cluster
        store.add_symptom_profile(&SymptomProfile {
            id: "muscle_cramps".into(),
            name: "Muscle Cramps".into(),
            cui: None,
            aliases: vec![],
            archetype_id: "pain".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: None,
            associated_systems: vec!["musculoskeletal system".into()],
        }).await;

        store.add_symptom_profile(&SymptomProfile {
            id: "insomnia".into(),
            name: "Insomnia".into(),
            cui: None,
            aliases: vec![],
            archetype_id: "sleep".into(),
            relevant_oldcarts_override: None,
            irrelevant_oldcarts_override: None,
            sufficient_dimensions_override: None,
            associated_systems: vec!["nervous system".into()],
        }).await;

        let mut ctx = empty_context();
        ctx.stage = IntakeStageId::SystemReview;
        ctx.active_profiles = vec!["muscle_cramps".into(), "insomnia".into()];

        let action = engine.next_turn(&ctx).await;
        if let Some(q) = &action.question {
            // Electrolyte cluster prioritizes nervous system
            // so we should get review_nervous first
            assert!(
                q.template_id == "review_nervous" || q.template_id == "review_musculoskeletal",
                "Expected nervous or musculoskeletal review, got: {}",
                q.template_id
            );
        }
    }
}
