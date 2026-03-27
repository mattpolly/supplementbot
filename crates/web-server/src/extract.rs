use llm_client::provider::{CompletionRequest, LlmProvider};
use serde::Deserialize;

use intake_agent::phase::UserSignal;
use intake_agent::session::IntakeSession;

// ---------------------------------------------------------------------------
// Extraction — cheap-model slot filler.
//
// Parses user text into structured data:
//   - Symptoms / systems mentioned
//   - OLDCARTS fields
//   - System affirm/deny
//   - Correction detection
//
// Uses a small, fixed prompt (~300 tokens) — no session context needed.
// This is the job that can run on Haiku/Flash while the renderer uses Sonnet.
// ---------------------------------------------------------------------------

/// What the extractor pulls from one user message.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Extraction {
    /// Symptom-like phrases (e.g., "muscle cramps", "can't sleep")
    #[serde(default)]
    pub symptoms: Vec<String>,
    /// System-like phrases (e.g., "digestive", "nervous")
    #[serde(default)]
    pub systems: Vec<String>,
    /// OLDCARTS field fills
    #[serde(default)]
    pub oldcarts: OldcartsExtraction,
    /// Did the user deny a system? (e.g., "no digestive issues")
    #[serde(default)]
    pub denied_systems: Vec<String>,
    /// Is this a correction of a previous statement?
    #[serde(default)]
    pub is_correction: bool,
    /// If correction, what was the old concept?
    #[serde(default)]
    pub correction_old: Option<String>,
    /// If correction, what is the new concept?
    #[serde(default)]
    pub correction_new: Option<String>,
    /// User engagement signal
    #[serde(default)]
    pub engagement: EngagementLevel,
    /// Medications or supplements the user mentioned taking
    #[serde(default)]
    pub medications: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct OldcartsExtraction {
    #[serde(default)]
    pub onset: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub duration: Option<String>,
    #[serde(default)]
    pub character: Option<String>,
    #[serde(default)]
    pub aggravating: Vec<String>,
    #[serde(default)]
    pub alleviating: Vec<String>,
    #[serde(default)]
    pub radiation: Option<String>,
    #[serde(default)]
    pub timing: Option<String>,
    #[serde(default)]
    pub severity: Option<u8>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EngagementLevel {
    #[default]
    Normal,
    Disengaged,
    WantsRecommendations,
    DoneSharing,
}

const EXTRACTION_SYSTEM_PROMPT: &str = r#"You are a clinical data extractor. Given a patient's message during a supplement intake interview, extract structured data. Respond ONLY with valid JSON matching this schema:

{
  "symptoms": ["symptom phrases mentioned"],
  "systems": ["body systems mentioned or implied"],
  "oldcarts": {
    "onset": "when it started, or null",
    "location": "where in the body, or null",
    "duration": "how long it lasts, or null",
    "character": "what it feels like, or null",
    "aggravating": ["things that make it worse"],
    "alleviating": ["things that make it better"],
    "radiation": "does it spread, or null",
    "timing": "pattern or time of day, or null",
    "severity": null or 1-10
  },
  "denied_systems": ["systems the user explicitly said are NOT a problem"],
  "is_correction": false,
  "correction_old": null,
  "correction_new": null,
  "engagement": "normal" or "disengaged" or "wants_recommendations" or "done_sharing",
  "medications": ["prescription drugs or supplements the user mentions taking"]
}

Rules:
- Only extract what the user explicitly stated. Do not infer.
- "denied_systems" = systems the user says are fine / not a problem.
- "engagement": "disengaged" if user gives very short dismissive answers like "idk", "not sure", "skip". "wants_recommendations" ONLY if they explicitly ask for supplement recommendations (e.g., "what should I take?", "just give me the recommendations"). "done_sharing" if the user signals they've said everything (e.g., "that's it", "that's all", "nothing else"). Off-topic questions or asking about non-supplement advice is "normal".
- "medications": any prescription drugs, OTC medications, or supplements the user says they are currently taking. Only extract what they explicitly state.
- For severity, only extract if they give a number.
- Return empty arrays/null for fields with no data."#;

/// Run the extraction LLM on a user message. Returns structured data.
pub async fn extract_from_message(
    user_message: &str,
    extractor: &dyn LlmProvider,
) -> Extraction {
    let request = CompletionRequest::new(user_message)
        .with_system(EXTRACTION_SYSTEM_PROMPT.to_string())
        .with_max_tokens(512)
        .with_temperature(0.0); // deterministic extraction

    match extractor.complete(request).await {
        Ok(response) => parse_extraction(&response.content),
        Err(e) => {
            eprintln!("[extract] LLM error: {e}");
            // Fall back to heuristic extraction
            heuristic_extraction(user_message)
        }
    }
}

/// Parse the LLM's JSON response into an Extraction.
fn parse_extraction(json_text: &str) -> Extraction {
    // Strip markdown code fences if present
    let cleaned = json_text
        .trim()
        .strip_prefix("```json")
        .or_else(|| json_text.trim().strip_prefix("```"))
        .unwrap_or(json_text.trim())
        .strip_suffix("```")
        .unwrap_or(json_text.trim())
        .trim();

    serde_json::from_str(cleaned).unwrap_or_else(|e| {
        eprintln!("[extract] JSON parse error: {e}");
        eprintln!("[extract] raw: {cleaned}");
        Extraction::default()
    })
}

/// Heuristic fallback when the LLM is unavailable.
/// Uses the intake-agent's built-in signal detection.
fn heuristic_extraction(text: &str) -> Extraction {
    let signal = intake_agent::phase::detect_signal(text);
    Extraction {
        engagement: match signal {
            UserSignal::Disengaged => EngagementLevel::Disengaged,
            UserSignal::WantsRecommendations => EngagementLevel::WantsRecommendations,
            UserSignal::Correction => {
                return Extraction {
                    is_correction: true,
                    ..Default::default()
                };
            }
            _ => EngagementLevel::Normal,
        },
        ..Default::default()
    }
}

/// Convert extraction engagement level to intake-agent UserSignal.
pub fn to_user_signal(extraction: &Extraction) -> UserSignal {
    if extraction.is_correction {
        return UserSignal::Correction;
    }
    match extraction.engagement {
        EngagementLevel::Disengaged => UserSignal::Disengaged,
        EngagementLevel::WantsRecommendations => UserSignal::WantsRecommendations,
        EngagementLevel::DoneSharing => UserSignal::DoneSharing,
        EngagementLevel::Normal => UserSignal::Normal,
    }
}

/// Apply extracted data to the session.
pub fn apply_extraction(session: &mut IntakeSession, extraction: &Extraction) {
    // Fill OLDCARTS fields (only overwrite None fields)
    if let Some(ref v) = extraction.oldcarts.onset {
        if session.oldcarts.onset.is_none() {
            session.oldcarts.onset = Some(v.clone());
        }
    }
    if let Some(ref v) = extraction.oldcarts.location {
        if session.oldcarts.location.is_none() {
            session.oldcarts.location = Some(v.clone());
        }
    }
    if let Some(ref v) = extraction.oldcarts.duration {
        if session.oldcarts.duration.is_none() {
            session.oldcarts.duration = Some(v.clone());
        }
    }
    if let Some(ref v) = extraction.oldcarts.character {
        if session.oldcarts.character.is_none() {
            session.oldcarts.character = Some(v.clone());
        }
    }
    for v in &extraction.oldcarts.aggravating {
        if !session.oldcarts.aggravating.contains(v) {
            session.oldcarts.aggravating.push(v.clone());
        }
    }
    for v in &extraction.oldcarts.alleviating {
        if !session.oldcarts.alleviating.contains(v) {
            session.oldcarts.alleviating.push(v.clone());
        }
    }
    if let Some(ref v) = extraction.oldcarts.radiation {
        if session.oldcarts.radiation.is_none() {
            session.oldcarts.radiation = Some(v.clone());
        }
    }
    if let Some(ref v) = extraction.oldcarts.timing {
        if session.oldcarts.timing.is_none() {
            session.oldcarts.timing = Some(v.clone());
        }
    }
    if let Some(v) = extraction.oldcarts.severity {
        if session.oldcarts.severity.is_none() {
            session.oldcarts.severity = Some(v);
        }
    }

    // Record denied systems
    for system in &extraction.denied_systems {
        session.deny_system(system.to_lowercase());
    }
}
