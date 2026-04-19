use axum::extract::State;
use axum::response::Json;
use uuid::Uuid;

use graph_service::intake::engine::{IntakeEngine, TraversalContext};
use graph_service::intake::executor::GraphActionExecutor;
use graph_service::intake::types::IntakeStageId;
use intake_agent::candidates::{Candidate, CandidateSet};
use intake_agent::concept_map;
use intake_agent::context;
use intake_agent::phase;
use intake_agent::safety::{self, FilterResult, SafetyCheck};
use intake_agent::session::IntakePhase;
use llm_client::provider::CompletionRequest;

use crate::extract::{apply_extraction, extract_from_message, to_user_signal};
use crate::state::AppState;
use crate::symptom_resolver;

// ---------------------------------------------------------------------------
// Per-turn orchestration — v2 (intake KG driven).
//
// The intake KG engine decides what to ask. The LLM renders it naturally.
//
//   1. Red flag check
//   2. Extract structured data (cheap model)
//   3. Record turn + apply extraction to session
//   4. Map concepts to graph nodes + symptom profiles
//   5. Build TraversalContext from session state
//   6. Engine: next_turn() → TurnAction (question, graph actions, stage)
//   7. Executor: run graph actions against supplement KG + iDISK
//   8. Update session (candidates, phase, lens, visited questions)
//   9. Build context v2 + call renderer LLM
//  10. Post-generation safety filter
// ---------------------------------------------------------------------------

/// A single PubMed citation to surface in the UI.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CitationRef {
    pub ingredient: String,
    pub pmid: u64,
    pub url: String,
    pub sentence: String,
    pub confidence: f64,
}

/// The result of processing one turn.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TurnResult {
    /// The agent's response text (safe to show to user)
    pub response: String,
    /// Current phase for UI state
    pub phase: String,
    /// Whether this is an emergency exit
    pub emergency: bool,
    /// Whether the session is complete (recommendation delivered)
    pub complete: bool,
    /// Current candidate count (for UI)
    pub candidate_count: usize,
    /// PubMed citations supporting the recommendation (populated at recommendation phase)
    pub citations: Vec<CitationRef>,
    /// LLM system prompt for this turn (only populated when DEBUG_LLM_PROMPT=true)
    pub debug_llm_prompt: Option<String>,
}

/// Process one user message through the full pipeline.
pub async fn process_turn(
    state: &AppState,
    session_id: &Uuid,
    user_message: &str,
) -> Option<TurnResult> {
    let s = &state.inner;

    // Step 1: Red flag check (no LLM, no graph)
    if let SafetyCheck::EmergencyExit(flag) = safety::check_red_flags(user_message) {
        eprintln!("[session {session_id}] RED FLAG: {flag}");
        s.sessions
            .with_session(session_id, |session| {
                session.add_user_turn(user_message);
            })
            .await?;
        s.sessions.remove_session(session_id).await;
        return Some(TurnResult {
            response: String::new(),
            phase: "emergency".to_string(),
            emergency: true,
            complete: true,
            candidate_count: 0,
            citations: vec![],
            debug_llm_prompt: None,
        });
    }

    // Step 2: Extract structured data (cheap model)
    let extraction = extract_from_message(user_message, s.extractor.as_ref()).await;
    let user_signal = to_user_signal(&extraction);

    eprintln!(
        "[session {session_id}] extraction: symptoms={:?} systems={:?} oldcarts_onset={:?} oldcarts_char={:?} denied={:?} engagement={:?}",
        extraction.symptoms,
        extraction.systems,
        extraction.oldcarts.onset,
        extraction.oldcarts.character,
        extraction.denied_systems,
        extraction.engagement,
    );

    // Step 3: Record user turn + apply extraction to session
    s.sessions
        .with_session(session_id, |session| {
            session.add_user_turn(user_message);

            // Handle corrections
            if extraction.is_correction {
                if let (Some(old), Some(new)) =
                    (&extraction.correction_old, &extraction.correction_new)
                {
                    session.revise_complaint(old, new);
                }
            }

            // Add new chief complaints (during CC phase)
            if session.phase == IntakePhase::ChiefComplaint && !extraction.symptoms.is_empty() {
                session.add_complaint(user_message);
                for symptom in &extraction.symptoms {
                    if let Some(cc) = session.chief_complaints.last_mut() {
                        cc.mapped_symptoms.push(symptom.to_lowercase());
                    }
                }
                for system in &extraction.systems {
                    if let Some(cc) = session.chief_complaints.last_mut() {
                        cc.mapped_systems.push(system.to_lowercase());
                    }
                }
            }

            // Apply OLDCARTS + denied systems
            apply_extraction(session, &extraction);

            // Record disclosed medications and supplements for contraindication checking
            if !extraction.medications.is_empty() {
                for med in &extraction.medications {
                    let lower = med.to_lowercase();
                    if !session.contraindications.contains(&lower) {
                        session.contraindications.push(lower.clone());
                    }
                    // Heuristic: if it's in supplement-like terms, also track as supplement
                    // (In v2, the executor will check the iDISK ingredient table)
                    if !session.disclosed_supplements.contains(&lower) {
                        session.disclosed_supplements.push(lower);
                    }
                }
            }

            eprintln!(
                "[session {}] oldcarts filled: {}/9 | profiles: {:?}",
                session.id,
                session.oldcarts.filled_count(),
                session.active_profiles,
            );
        })
        .await?;

    // Step 4: Map concepts to graph nodes (for body systems, mechanisms, etc.)
    for symptom in &extraction.symptoms {
        let mapped = concept_map::map_text_to_nodes(symptom, &s.graph, &s.merge).await;
        if mapped.is_empty() {
            concept_map::log_unmapped(symptom, &format!("session {session_id}"));
        }
    }

    // Step 4a: Symptom resolver — maps free-text symptom phrases to intake
    // profile IDs using a closed-vocabulary LLM classifier. This handles
    // colloquial terms ("jittery", "queasy", "can't think straight") that
    // alias lists and string matching can't reliably cover, while respecting
    // clinical distinctions between similar-sounding profiles.
    if !extraction.symptoms.is_empty() {
        let known_profiles = s.intake_store.all_symptom_profile_ids().await;
        let resolved = symptom_resolver::resolve_symptoms(
            &extraction.symptoms,
            &known_profiles,
            s.extractor.as_ref(),
        )
        .await;

        eprintln!(
            "[session {session_id}] symptom resolver: {:?} → {:?}",
            extraction.symptoms, resolved
        );

        if !resolved.is_empty() {
            s.sessions
                .with_session(session_id, |session| {
                    for profile_id in &resolved {
                        if !session.active_profiles.contains(profile_id) {
                            session.active_profiles.push(profile_id.clone());
                        }
                    }
                })
                .await;
        }
    }

    // Step 5: Build TraversalContext from session state
    let traversal_ctx = s
        .sessions
        .with_session(session_id, |session| {
            let filled_dims = session.oldcarts.filled_dimensions();
            let filled_count = session.oldcarts.filled_count();

            // Compute confidence gap from current candidates
            let (top_score, confidence_gap) = if session.candidates.len() >= 2 {
                let top = session.candidates.top(2);
                (
                    top[0].composite_score,
                    top[0].composite_score - top[1].composite_score,
                )
            } else if session.candidates.len() == 1 {
                (session.candidates.top(1)[0].composite_score, 0.0)
            } else {
                (0.0, 0.0)
            };

            let candidate_names: Vec<String> = session
                .candidates
                .top(10)
                .iter()
                .map(|c| c.ingredient.clone())
                .collect();

            TraversalContext {
                stage: phase_to_stage(&session.phase),
                active_profiles: session.active_profiles.clone(),
                filled_oldcarts: filled_dims,
                filled_count,
                visited_questions: session.visited_questions.clone(),
                goal_ask_counts: session.goal_ask_counts.clone(),
                candidate_names,
                candidate_count: session.candidates.len(),
                confidence_gap,
                top_score,
                reviewed_systems: session.systems_reviewed.clone(),
                checklist_complete: session.checklist.complete(),
                checklist_next_question: session.checklist.next_required_question(),
                contraindications_ready: session.checklist.contraindications_ready(),
                contraindications_checked: session.checklist.contraindications_checked,
                medications_disclosed: !session.contraindications.is_empty(),
                user_disengaged: user_signal == phase::UserSignal::Disengaged,
                user_done_sharing: user_signal == phase::UserSignal::DoneSharing,
                differentiator_count: session.last_differentiator_count,
                chief_complaint: session
                    .chief_complaints
                    .first()
                    .map(|cc| cc.raw_text.clone()),
                differentiation_turns: session.differentiation_turns,
            }
        })
        .await?;

    // Step 6: Engine — determine next action
    let engine = IntakeEngine::new(&s.intake_store);
    let turn_action = engine.next_turn(&traversal_ctx).await;

    eprintln!(
        "[session {session_id}] engine trace: {}",
        turn_action.trace.join(" → ")
    );

    // Get relevant dimensions for context building
    let relevant_dims: Vec<_> = engine
        .relevant_dimensions(&traversal_ctx)
        .await
        .into_iter()
        .collect();

    // Step 7: Executor — run graph actions against supplement KG + iDISK
    let executor = GraphActionExecutor::new(&s.graph, &s.source, &s.merge, &s.idisk);

    let all_symptoms: Vec<String> = s
        .sessions
        .with_session(session_id, |session| {
            session
                .all_symptoms()
                .into_iter()
                .map(|s| s.to_string())
                .collect()
        })
        .await?;

    let (candidate_names, disclosed_meds, disclosed_supps, lens_level) = s
        .sessions
        .with_session(session_id, |session| {
            let names: Vec<String> = session
                .candidates
                .top(10)
                .iter()
                .map(|c| c.ingredient.clone())
                .collect();
            (
                names,
                session.contraindications.clone(),
                session.disclosed_supplements.clone(),
                session.lens_level,
            )
        })
        .await?;

    let action_results = executor
        .execute(
            &turn_action.graph_actions,
            &all_symptoms,
            &candidate_names,
            &disclosed_meds,
            &disclosed_supps,
            lens_level,
        )
        .await;

    // Step 8: Update session — candidates, phase, lens, visited questions
    let new_phase = s
        .sessions
        .with_session(session_id, |session| {
            // Convert executor candidates to CandidateSet
            let new_candidates = CandidateSet {
                candidates: action_results
                    .candidates
                    .iter()
                    .map(|cr| Candidate {
                        ingredient: cr.ingredient.clone(),
                        per_symptom_scores: std::collections::HashMap::new(),
                        composite_score: cr.score,
                        supporting_paths: vec![],
                        quality: parse_quality(&cr.quality),
                        contraindications: vec![],
                    })
                    .collect(),
            };

            // Only update candidates if executor returned results
            if !new_candidates.is_empty() {
                session.candidates = new_candidates;
            }

            // Record differentiator count for next turn's traversal context
            session.last_differentiator_count = action_results.discriminators.len();

            // Track turns spent in differentiation phase
            if session.phase == IntakePhase::Differentiation {
                session.differentiation_turns += 1;
            }

            // Track visited question and update safety checklist.
            // Flags are set ONLY when the engine delivers the specific template —
            // never from user-volunteered information (liability requirement).
            if let Some(ref q) = turn_action.question {
                session.visited_questions.insert(q.template_id.clone());
                *session
                    .goal_ask_counts
                    .entry(q.goal_id.clone())
                    .or_insert(0) += 1;
                match q.template_id.as_str() {
                    "ask_prescriptions" => session.checklist.prescriptions_asked = true,
                    "ask_otc_supplements" => session.checklist.otc_and_supplements_asked = true,
                    "ask_health_conditions" => session.checklist.health_conditions_asked = true,
                    _ => {}
                }
            }

            // Run contraindication check once all prerequisites are met.
            // This is the last checklist item — it runs automatically, no question needed.
            if session.checklist.contraindications_ready()
                && !session.checklist.contraindications_checked
            {
                // The actual iDISK interaction check ran in the executor above —
                // any flagged contraindications are already in action_results.
                // We just mark the check as done so the gate unlocks.
                session.checklist.contraindications_checked = true;
            }

            // Apply stage transition from engine
            if let Some(ref next_stage) = turn_action.next_stage {
                session.phase = stage_to_phase(next_stage);
            }

            // Update lens level
            let new_lens = phase::compute_lens_level(session);
            session.escalate_lens(new_lens);

            session.phase.clone()
        })
        .await?;

    // Step 9: Build context v2 + call renderer LLM
    let intake_context = s
        .sessions
        .with_session(session_id, |session| {
            context::build_context_v2(session, &turn_action, &action_results, &relevant_dims)
        })
        .await?;

    let llm_request = CompletionRequest::new(&intake_context.user_message)
        .with_system(intake_context.system_prompt.clone())
        .with_max_tokens(if new_phase == IntakePhase::Recommendation {
            1024
        } else {
            400
        })
        .with_temperature(0.6);

    let llm_response = match s.renderer.complete(llm_request).await {
        Ok(resp) => resp.content,
        Err(e) => {
            eprintln!("[session {session_id}] renderer error: {e}");
            "I'm sorry, I'm having trouble right now. Could you try again?".to_string()
        }
    };

    // Step 10: Post-generation safety filter
    let safe_response = match s.safety_filter.check(&llm_response) {
        FilterResult::Pass(text) => text,
        FilterResult::Rewrite {
            original: _,
            violation,
        } => {
            eprintln!("[session {session_id}] safety rewrite: {violation}");
            "Based on your symptoms, there are some supplements that act on the relevant \
             body systems. Let me gather a bit more information to narrow things down."
                .to_string()
        }
        FilterResult::Block { violation } => {
            eprintln!("[session {session_id}] safety BLOCK: {violation}");
            "I want to make sure I'm being helpful and accurate. Let me rephrase — \
             there are supplements that may support the systems where your symptoms present. \
             Can you tell me more about what you're experiencing?"
                .to_string()
        }
    };

    let complete = new_phase == IntakePhase::Recommendation;

    let candidate_count = s
        .sessions
        .with_session(session_id, |session| {
            session.add_agent_turn(&safe_response);
            session.candidates.len()
        })
        .await
        .unwrap_or(0);

    let phase_str = match new_phase {
        IntakePhase::ChiefComplaint => "chief_complaint",
        IntakePhase::Hpi => "hpi",
        IntakePhase::ReviewOfSystems => "review_of_systems",
        IntakePhase::Differentiation => "differentiation",
        IntakePhase::CausationInquiry => "causation_inquiry",
        IntakePhase::Recommendation => "recommendation",
    };

    // At recommendation phase, look up PubMed citations for each candidate.
    let citations = if complete {
        gather_citations(state, session_id).await
    } else {
        vec![]
    };

    let debug_llm_prompt = if state.inner.debug_llm_prompt {
        Some(intake_context.system_prompt.clone())
    } else {
        None
    };

    Some(TurnResult {
        response: safe_response,
        phase: phase_str.to_string(),
        emergency: false,
        complete,
        candidate_count,
        citations,
        debug_llm_prompt,
    })
}

// ---------------------------------------------------------------------------
// Citation lookup
// ---------------------------------------------------------------------------

/// For each top candidate, resolve its CUI via the merge store and look up
/// PubMed citations from SuppKG. Returns up to 3 citations per ingredient,
/// sorted by confidence, deduped by PMID.
async fn gather_citations(state: &AppState, session_id: &Uuid) -> Vec<CitationRef> {
    let suppkg = match &state.inner.suppkg {
        Some(kg) => kg.clone(),
        None => return vec![],
    };

    let candidates: Vec<String> = match state.inner.sessions
        .with_session(session_id, |session| {
            session.candidates.top(10).iter().map(|c| c.ingredient.clone()).collect()
        })
        .await
    {
        Some(c) => c,
        None => return vec![],
    };

    let mut result = Vec::new();

    for ingredient in &candidates {
        // Resolve ingredient name → CUI via merge store
        let ingredient_cui = match state.inner.merge.cui_for(ingredient).await {
            Some(cui) => cui,
            None => {
                // Fall back to SuppKG's own term index
                match suppkg.resolve_cui(ingredient) {
                    Some(m) => m.cui,
                    None => continue,
                }
            }
        };

        // Get all outgoing edges from this ingredient CUI in SuppKG
        let outgoing = suppkg.outgoing_edges(&ingredient_cui);

        let mut seen_pmids = std::collections::HashSet::new();
        let mut ingredient_citations: Vec<CitationRef> = Vec::new();

        for (target_cui, _predicate) in outgoing {
            let cites = suppkg.citations_for(&ingredient_cui, target_cui, None);
            for cite in cites {
                if cite.pmid == 0 { continue; }
                if seen_pmids.contains(&cite.pmid) { continue; }
                seen_pmids.insert(cite.pmid);
                ingredient_citations.push(CitationRef {
                    ingredient: ingredient.clone(),
                    pmid: cite.pmid,
                    url: format!("https://pubmed.ncbi.nlm.nih.gov/{}/", cite.pmid),
                    sentence: cite.sentence.clone(),
                    confidence: cite.confidence,
                });
            }
        }

        // Sort by confidence descending, take top 3 per ingredient
        ingredient_citations.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));
        ingredient_citations.truncate(3);
        result.extend(ingredient_citations);
    }

    result
}

// ---------------------------------------------------------------------------
// Phase ↔ Stage conversion
// ---------------------------------------------------------------------------

fn phase_to_stage(phase: &IntakePhase) -> IntakeStageId {
    match phase {
        IntakePhase::ChiefComplaint => IntakeStageId::ChiefComplaint,
        IntakePhase::Hpi => IntakeStageId::Hpi,
        IntakePhase::ReviewOfSystems => IntakeStageId::SystemReview,
        IntakePhase::Differentiation => IntakeStageId::Differentiation,
        IntakePhase::CausationInquiry => IntakeStageId::CausationInquiry,
        IntakePhase::Recommendation => IntakeStageId::Recommendation,
    }
}

fn stage_to_phase(stage: &IntakeStageId) -> IntakePhase {
    match stage {
        IntakeStageId::ChiefComplaint => IntakePhase::ChiefComplaint,
        IntakeStageId::Hpi => IntakePhase::Hpi,
        IntakeStageId::SystemReview => IntakePhase::ReviewOfSystems,
        IntakeStageId::Differentiation => IntakePhase::Differentiation,
        IntakeStageId::CausationInquiry => IntakePhase::CausationInquiry,
        IntakeStageId::Recommendation => IntakePhase::Recommendation,
    }
}

fn parse_quality(quality_str: &str) -> graph_service::source::EdgeQuality {
    use graph_service::source::EdgeQuality;
    match quality_str {
        s if s.contains("CitationBacked") => EdgeQuality::CitationBacked,
        s if s.contains("MultiProvider") => EdgeQuality::MultiProvider,
        s if s.contains("SingleProvider") => EdgeQuality::SingleProvider,
        s if s.contains("Speculative") => EdgeQuality::Speculative,
        _ => EdgeQuality::Deduced,
    }
}

// ---------------------------------------------------------------------------
// REST endpoints
// ---------------------------------------------------------------------------

pub async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let stats = state.inner.sessions.stats().await;
    Json(serde_json::json!({
        "status": "ok",
        "renderer": state.inner.renderer.model_name(),
        "extractor": state.inner.extractor.model_name(),
        "sessions": stats,
    }))
}

pub async fn stats(State(state): State<AppState>) -> Json<serde_json::Value> {
    let stats = state.inner.sessions.stats().await;
    Json(serde_json::json!(stats))
}
