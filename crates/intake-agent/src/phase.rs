use crate::differentiator::Differentiator;
use crate::session::{IntakePhase, IntakeSession};

// ---------------------------------------------------------------------------
// Phase transition logic — the state machine driving the conversation.
//
// CC → HPI → ROS → Differentiation (loop) → Recommendation
//
// Transitions are not strictly linear. Differentiation loops back on itself
// as long as high-value differentiators remain AND the user is engaged.
//
// Phase transitions are driven by session state, not by turn count.
// ---------------------------------------------------------------------------

/// Signals from user behavior that inform phase transitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserSignal {
    /// User gave a substantive, engaged answer
    Engaged,
    /// User gave a short or dismissive answer ("I don't know", "not sure", etc.)
    Disengaged,
    /// User explicitly asked for recommendations
    WantsRecommendations,
    /// User is correcting a previous statement
    Correction,
    /// User indicated they're done sharing ("that's it", "that's all", "nothing else")
    DoneSharing,
    /// Normal conversational input (default)
    Normal,
}

/// Evaluate the next phase based on current session state.
/// Returns the phase the session should transition to (may be the same phase).
pub fn evaluate_transition(
    session: &IntakeSession,
    differentiators: &[Differentiator],
    unreviewed_system_count: usize,
    user_signal: &UserSignal,
) -> IntakePhase {
    let candidate_phase = evaluate_transition_inner(
        session,
        differentiators,
        unreviewed_system_count,
        user_signal,
    );

    // Safety gate: before entering Recommendation, we MUST have asked about
    // current medications and supplements. Interactions are safety-critical.
    if candidate_phase == IntakePhase::Recommendation && !session.checklist.complete() {
        return session.phase.clone();
    }

    candidate_phase
}

fn evaluate_transition_inner(
    session: &IntakeSession,
    differentiators: &[Differentiator],
    unreviewed_system_count: usize,
    user_signal: &UserSignal,
) -> IntakePhase {
    // User explicitly wants recommendations or is done sharing → skip ahead,
    // but only if we actually have candidates to recommend.
    if (*user_signal == UserSignal::WantsRecommendations
        || *user_signal == UserSignal::DoneSharing)
        && !session.candidates.is_empty()
    {
        return IntakePhase::Recommendation;
    }

    // Confidence-based auto-recommend: if the top candidate is far ahead of #2,
    // and we've gathered enough info (at least in HPI or later), go to recommendation.
    if session.phase != IntakePhase::ChiefComplaint && session.candidates.len() >= 1 {
        let top = &session.candidates.candidates;
        let clear_winner = match top.len() {
            0 => false,
            1 => top[0].composite_score > 0.5, // only candidate, decent score
            _ => {
                let gap = top[0].composite_score - top[1].composite_score;
                gap > 0.3 * top[0].composite_score // >30% gap between #1 and #2
            }
        };
        // Only auto-recommend if we've done some OLDCARTS and there's a clear winner
        if clear_winner && session.oldcarts.filled_count() >= 3 {
            return IntakePhase::Recommendation;
        }
    }

    match session.phase {
        IntakePhase::ChiefComplaint => {
            if !session.chief_complaints.is_empty() {
                IntakePhase::Hpi
            } else {
                IntakePhase::ChiefComplaint
            }
        }

        IntakePhase::Hpi => {
            if session.oldcarts.filled_count() >= 5
                || *user_signal == UserSignal::Disengaged
            {
                if !session.candidates.is_empty() && unreviewed_system_count > 0 {
                    IntakePhase::ReviewOfSystems
                } else if !session.candidates.is_empty() && !differentiators.is_empty() {
                    IntakePhase::Differentiation
                } else if !session.candidates.is_empty() {
                    IntakePhase::Recommendation
                } else {
                    IntakePhase::Hpi
                }
            } else {
                IntakePhase::Hpi
            }
        }

        IntakePhase::ReviewOfSystems => {
            if unreviewed_system_count == 0 || *user_signal == UserSignal::Disengaged {
                if !differentiators.is_empty() {
                    IntakePhase::Differentiation
                } else {
                    IntakePhase::Recommendation
                }
            } else {
                IntakePhase::ReviewOfSystems
            }
        }

        IntakePhase::Differentiation => {
            if differentiators.is_empty() || *user_signal == UserSignal::Disengaged {
                IntakePhase::Recommendation
            } else {
                IntakePhase::Differentiation
            }
        }

        IntakePhase::CausationInquiry => {
            // Always move to recommendation after causation inquiry
            IntakePhase::Recommendation
        }

        IntakePhase::Recommendation => {
            IntakePhase::Recommendation
        }
    }
}

/// Detect user engagement signals from their message text.
/// This is a simple heuristic for v1 — the LLM will handle nuance.
pub fn detect_signal(text: &str) -> UserSignal {
    let lower = text.to_lowercase().trim().to_string();

    // Check for explicit recommendation request
    if lower.contains("recommend")
        || lower.contains("what should i take")
        || lower.contains("what do you suggest")
        || lower.contains("what supplements")
        || lower.contains("just tell me")
    {
        return UserSignal::WantsRecommendations;
    }

    // Check for "done sharing" — user has said everything they want to say
    if lower.contains("that's it")
        || lower.contains("that's all")
        || lower.contains("thats it")
        || lower.contains("thats all")
        || lower.contains("nothing else")
        || lower.contains("that covers it")
        || lower.contains("i think that's everything")
        || lower.contains("no, that's it")
        || lower.contains("nope, that's it")
    {
        return UserSignal::DoneSharing;
    }

    // Check for disengagement (before corrections — "not sure" starts with "not ")
    if lower.len() < 15
        && (lower.contains("don't know")
            || lower.contains("not sure")
            || lower.contains("no idea")
            || lower == "idk"
            || lower == "no"
            || lower == "nope"
            || lower == "nothing"
            || lower == "skip"
            || lower == "next")
    {
        return UserSignal::Disengaged;
    }

    // Check for corrections
    if lower.starts_with("actually")
        || lower.starts_with("no, ")
        || lower.starts_with("not ")
        || lower.contains("i meant")
        || lower.contains("i mean ")
        || lower.contains("correction")
    {
        return UserSignal::Correction;
    }

    // Short answers (< 3 words) may indicate disengagement
    let word_count = lower.split_whitespace().count();
    if word_count <= 2 && !lower.contains("yes") {
        return UserSignal::Disengaged;
    }

    UserSignal::Normal
}

/// Compute the lens level based on session state.
/// The lens escalates as more clinical detail is gathered.
pub fn compute_lens_level(session: &IntakeSession) -> f64 {
    let mut level = 0.15; // baseline: 5th-grade

    // Each OLDCARTS dimension filled raises the lens slightly
    let filled = session.oldcarts.filled_count();
    level += filled as f64 * 0.04; // 9 dimensions × 0.04 = 0.36 max from OLDCARTS

    // ROS review raises the lens
    let reviewed = session.systems_reviewed.len();
    level += (reviewed as f64 * 0.03).min(0.15);

    // Differentiation / CausationInquiry phase gets even higher
    if session.phase == IntakePhase::Differentiation
        || session.phase == IntakePhase::CausationInquiry
    {
        level += 0.1;
    }

    level.clamp(0.15, 0.85) // never go fully expert in an intake
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn empty_session() -> IntakeSession {
        IntakeSession::new()
    }

    #[test]
    fn test_initial_phase_stays_cc() {
        let session = empty_session();
        let next = evaluate_transition(&session, &[], 0, &UserSignal::Normal);
        assert_eq!(next, IntakePhase::ChiefComplaint);
    }

    #[test]
    fn test_cc_to_hpi_on_complaint() {
        let mut session = empty_session();
        session.add_complaint("my legs hurt");
        let next = evaluate_transition(&session, &[], 0, &UserSignal::Normal);
        assert_eq!(next, IntakePhase::Hpi);
    }

    #[test]
    fn test_explicit_recommendation_request_without_candidates_stays() {
        // No candidates → can't recommend, stay in current phase
        let session = empty_session();
        let next = evaluate_transition(&session, &[], 5, &UserSignal::WantsRecommendations);
        assert_eq!(next, IntakePhase::ChiefComplaint);
    }

    #[test]
    fn test_explicit_recommendation_request_with_candidates_skips() {
        use crate::candidates::{Candidate, CandidateSet};
        use graph_service::source::EdgeQuality;
        let mut session = empty_session();
        session.add_complaint("headache");
        session.checklist.prescriptions_asked = true;
        session.checklist.otc_and_supplements_asked = true;
        session.checklist.health_conditions_asked = true;
        session.checklist.contraindications_checked = true;
        session.candidates = CandidateSet {
            candidates: vec![Candidate {
                ingredient: "magnesium".to_string(),
                per_symptom_scores: HashMap::new(),
                composite_score: 0.8,
                supporting_paths: vec![],
                quality: EdgeQuality::MultiProvider,
                contraindications: vec![],
            }],
        };
        let next = evaluate_transition(&session, &[], 0, &UserSignal::WantsRecommendations);
        assert_eq!(next, IntakePhase::Recommendation);
    }

    #[test]
    fn test_recommendation_blocked_without_medication_check() {
        use crate::candidates::{Candidate, CandidateSet};
        use graph_service::source::EdgeQuality;
        let mut session = empty_session();
        session.add_complaint("headache");
        session.phase = IntakePhase::Differentiation;
        session.candidates = CandidateSet {
            candidates: vec![Candidate {
                ingredient: "magnesium".to_string(),
                per_symptom_scores: HashMap::new(),
                composite_score: 0.8,
                supporting_paths: vec![],
                quality: EdgeQuality::MultiProvider,
                contraindications: vec![],
            }],
        };
        // Without checklist complete, should stay in current phase
        let next = evaluate_transition(&session, &[], 0, &UserSignal::WantsRecommendations);
        assert_eq!(next, IntakePhase::Differentiation);
    }

    #[test]
    fn test_detect_signal_recommendation() {
        assert_eq!(
            detect_signal("What supplements should I take?"),
            UserSignal::WantsRecommendations,
        );
    }

    #[test]
    fn test_detect_signal_disengaged() {
        assert_eq!(detect_signal("idk"), UserSignal::Disengaged);
        assert_eq!(detect_signal("not sure"), UserSignal::Disengaged);
    }

    #[test]
    fn test_detect_signal_correction() {
        assert_eq!(
            detect_signal("actually it's more of a tingling"),
            UserSignal::Correction,
        );
    }

    #[test]
    fn test_detect_signal_normal() {
        assert_eq!(
            detect_signal("It started about two weeks ago after I began running more"),
            UserSignal::Normal,
        );
    }

    #[test]
    fn test_lens_level_escalation() {
        let mut session = empty_session();
        assert!((compute_lens_level(&session) - 0.15).abs() < 0.01);

        session.oldcarts.onset = Some("2 weeks ago".to_string());
        session.oldcarts.location = Some("legs".to_string());
        session.oldcarts.duration = Some("constant".to_string());
        session.oldcarts.character = Some("cramping".to_string());
        session.oldcarts.severity = Some(6);

        let level = compute_lens_level(&session);
        assert!(level > 0.3); // 5 fields × 0.04 = 0.20 + base 0.15 = 0.35
    }

    #[test]
    fn test_hpi_to_ros_on_sufficient_oldcarts() {
        let mut session = empty_session();
        session.add_complaint("muscle cramps");
        session.phase = IntakePhase::Hpi;
        session.oldcarts.onset = Some("2 weeks".to_string());
        session.oldcarts.location = Some("legs".to_string());
        session.oldcarts.duration = Some("constant".to_string());
        session.oldcarts.character = Some("cramping".to_string());
        session.oldcarts.severity = Some(6);

        // Need candidates and unreviewed systems to go to ROS
        // Without candidates, stays in HPI
        let next = evaluate_transition(&session, &[], 3, &UserSignal::Normal);
        assert_eq!(next, IntakePhase::Hpi);
    }

    #[test]
    fn test_differentiation_to_recommendation_when_exhausted() {
        let mut session = empty_session();
        session.phase = IntakePhase::Differentiation;
        session.checklist.prescriptions_asked = true;
        session.checklist.otc_and_supplements_asked = true;
        session.checklist.health_conditions_asked = true;
        session.checklist.contraindications_checked = true;

        let next = evaluate_transition(&session, &[], 0, &UserSignal::Normal);
        assert_eq!(next, IntakePhase::Recommendation);
    }
}
