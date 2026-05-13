use llm_client::provider::{CompletionRequest, LlmProvider};
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Planner — one fast Haiku call between extraction and rendering.
//
// Takes session state + extraction result, returns a structured decision:
//   - what to acknowledge (correction, repeat, frustration)
//   - which OLDCARTS topics were answered in summaries but not in state
//   - whether to skip any dimensions
//
// The renderer gets this decision and has one job: write natural prose
// that executes it. Clinical reasoning happens here, not there.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PlannerDecision {
    /// Something the renderer should acknowledge before asking the next
    /// question. Empty string = nothing to acknowledge.
    #[serde(default)]
    pub acknowledge: String,

    /// OLDCARTS topics the user has answered (per turn summaries) even if
    /// the extractor didn't fill the structured field.
    /// Values match the labels used in ALREADY COVERED:
    /// "onset", "location", "duration", "character", "aggravating",
    /// "alleviating", "radiation", "timing", "severity"
    #[serde(default)]
    pub topics_covered_in_conversation: Vec<String>,

    /// Dimensions that are clearly irrelevant for this complaint and should
    /// be skipped (e.g. "radiation" for tension headache).
    #[serde(default)]
    pub skip_dimensions: Vec<String>,

    /// Non-OLDCARTS topics the user has already addressed in conversation
    /// that the renderer must not ask about again. Free-text phrases,
    /// e.g. "allergies and sensitivities", "sleep quality", "stress levels".
    #[serde(default)]
    pub do_not_ask_again: Vec<String>,
}

const PLANNER_SYSTEM_PROMPT: &str = r#"You are a clinical interview planner. Given a summary of a supplement intake conversation, produce a JSON decision to guide the next response. Respond ONLY with valid JSON matching this schema:

{
  "acknowledge": "",
  "topics_covered_in_conversation": [],
  "skip_dimensions": [],
  "do_not_ask_again": []
}

Field definitions:

"acknowledge": A short instruction (one clause) for the responder if something needs addressing — e.g. "user pointed out sleep position was already asked", "user is frustrated by repeated questions", "user corrected the interviewer's process". Empty string if nothing to acknowledge.

"topics_covered_in_conversation": OLDCARTS topics the user has clearly answered somewhere in the turn summaries, even if not captured in structured fields. Use these exact strings: "onset", "location", "duration", "character", "aggravating", "alleviating", "radiation", "timing", "severity". Only include a topic if the summaries make clear it was answered.

"skip_dimensions": OLDCARTS dimensions that are clearly irrelevant for this specific complaint and should not be asked. Use the same strings as above. For example, "radiation" is rarely relevant for tension headache; "location" may already be implicit.

"do_not_ask_again": Non-OLDCARTS topics the user has already addressed in conversation that must not be asked again. Scan the turn summaries for any topic that was asked and answered — e.g. "allergies and sensitivities", "sleep quality", "stress levels", "diet restrictions", "supplement sensitivities", "neck and shoulder tension", "sleep position". Be thorough — include every topic already covered, not just the most recent one. Use short descriptive phrases (3-5 words max).

Rules:
- Be conservative with topics_covered_in_conversation — only include if you are confident the user answered it.
- Be conservative with skip_dimensions — only skip if clearly irrelevant.
- Be thorough with do_not_ask_again — include ALL non-OLDCARTS topics the user already addressed.
- acknowledge should be a brief instruction to the responder, not a full sentence the user will see.
- Return empty arrays/strings for fields with no data."#;

/// Run the planner. Returns a decision the renderer will execute.
pub async fn plan_turn(
    phase: &str,
    chief_complaint: &str,
    turn_summaries: &[String],
    oldcarts_filled: &[&str],
    last_bot_question: Option<&str>,
    checklist_prescriptions: bool,
    checklist_conditions: bool,
    checklist_supplements: bool,
    extractor: &dyn LlmProvider,
) -> PlannerDecision {
    if turn_summaries.is_empty() {
        return PlannerDecision::default();
    }

    let summaries_text = turn_summaries
        .iter()
        .enumerate()
        .map(|(i, s)| format!("  Turn {}: {}", i + 1, s))
        .collect::<Vec<_>>()
        .join("\n");

    let oldcarts_text = if oldcarts_filled.is_empty() {
        "  (none filled yet)".to_string()
    } else {
        oldcarts_filled.iter().map(|s| format!("  - {}", s)).collect::<Vec<_>>().join("\n")
    };

    let last_q_text = last_bot_question
        .map(|q| format!("Last bot question: {}", q))
        .unwrap_or_else(|| "Last bot question: (none)".to_string());

    let checklist_text = format!(
        "Prescriptions asked: {}\nHealth conditions asked: {}\nOTC/supplements asked: {}",
        checklist_prescriptions, checklist_conditions, checklist_supplements
    );

    let input = format!(
        "Phase: {}\nChief complaint: {}\n\n{}\n\nOLDCARTS already in structured state:\n{}\n\nTurn summaries:\n{}\n\nChecklist:\n{}",
        phase, chief_complaint, last_q_text, oldcarts_text, summaries_text, checklist_text
    );

    let request = CompletionRequest::new(&input)
        .with_system(PLANNER_SYSTEM_PROMPT.to_string())
        .with_max_tokens(256)
        .with_temperature(0.0);

    match extractor.complete(request).await {
        Ok(resp) => parse_decision(&resp.content),
        Err(e) => {
            eprintln!("[planner] LLM error: {e}");
            PlannerDecision::default()
        }
    }
}

fn parse_decision(json_text: &str) -> PlannerDecision {
    let cleaned = json_text
        .trim()
        .strip_prefix("```json")
        .or_else(|| json_text.trim().strip_prefix("```"))
        .unwrap_or(json_text.trim())
        .strip_suffix("```")
        .unwrap_or(json_text.trim())
        .trim();

    serde_json::from_str(cleaned).unwrap_or_else(|e| {
        eprintln!("[planner] JSON parse error: {e}");
        eprintln!("[planner] raw: {cleaned}");
        PlannerDecision::default()
    })
}
