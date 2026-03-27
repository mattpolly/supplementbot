use crate::differentiator::Differentiator;
use crate::session::{IntakePhase, IntakeSession, TurnRole};

use graph_service::intake::executor::ActionResults;
use graph_service::intake::types::{IntakeStageId, OldcartsDimension, TurnAction};

// ---------------------------------------------------------------------------
// Context generator — rebuilds the LLM system prompt each turn.
//
// This is the "regrounding" mechanism. The LLM is a language renderer,
// NOT the reasoner. The graph topology + session state determine what to
// ask. The LLM turns that into natural conversation.
//
// Returns a plain String — model-agnostic, no provider-specific tooling.
// ---------------------------------------------------------------------------

/// The complete context for one LLM turn.
pub struct IntakeContext {
    /// Full system prompt rebuilt from current session state.
    pub system_prompt: String,
    /// The user's latest message (for the LLM's user turn).
    pub user_message: String,
}

/// Build a fresh system prompt from current session state.
pub fn build_context(
    session: &IntakeSession,
    differentiators: &[Differentiator],
    unreviewed_systems: &[String],
) -> IntakeContext {
    let mut prompt = String::with_capacity(4096);

    // --- Role ---
    prompt.push_str(
        "ROLE:\n\
         You are a supplement intake specialist. You gather information about\n\
         a person's symptoms to identify supplements that may help.\n\n",
    );

    // --- Communication style ---
    prompt.push_str(
        "COMMUNICATION STYLE:\n\
         - Keep responses short — a few sentences. No walls of text.\n\
         - Sound like a warm, competent human. Not a textbook.\n\
         - One question per turn. Let the person answer before asking more.\n\
         - No bullet points, no headers, no markdown formatting, no emoji.\n\
         - Use plain language, not medical jargon.\n\
         - For recommendations (final phase only), you may be more thorough.\n\n",
    );

    // --- Legal constraints ---
    prompt.push_str(
        "LEGAL CONSTRAINTS:\n\
         - Never diagnose. Never say \"you have X.\"\n\
         - Never say \"cure.\" Supplements address symptoms, not diseases.\n\
         - Never give instructions like \"take X\" or \"you should try X.\"\n\
           Instead, report what the research suggests: \"For symptoms like\n\
           yours, the literature suggests X may help support...\" You are\n\
           reporting findings, not prescribing.\n\
         - If the user describes an emergency, direct them to call 911.\n\
         - Never recommend specific dosages.\n\n",
    );

    // --- Current phase ---
    prompt.push_str("CURRENT PHASE: ");
    prompt.push_str(phase_description(&session.phase));
    prompt.push_str("\n\n");

    // --- OLDCARTS mnemonic ---
    prompt.push_str(
        "MNEMONIC — OLDCARTS (Review of Symptoms):\n\
         O: Onset — When did this start?\n\
         L: Location — Where exactly?\n\
         D: Duration — How long does it last?\n\
         C: Character — What does it feel like?\n\
         A: Aggravating/Alleviating — What makes it better/worse?\n\
         R: Radiation — Does it spread?\n\
         T: Timing — Pattern? Time of day?\n\
         S: Severity — 1-10?\n\n",
    );

    // --- Gathered so far ---
    prompt.push_str("GATHERED SO FAR:\n");
    format_oldcarts(&session.oldcarts, &mut prompt);
    prompt.push('\n');

    // --- Chief complaints ---
    prompt.push_str("CHIEF COMPLAINTS:\n");
    if session.chief_complaints.is_empty() {
        prompt.push_str("  (none yet — this is the opening question)\n");
    } else {
        for (i, cc) in session.chief_complaints.iter().enumerate() {
            prompt.push_str(&format!("  {}. \"{}\"", i + 1, cc.raw_text));
            if !cc.mapped_symptoms.is_empty() {
                prompt.push_str(&format!(
                    " → symptoms: [{}]",
                    cc.mapped_symptoms.join(", ")
                ));
            }
            if !cc.mapped_systems.is_empty() {
                prompt.push_str(&format!(" → systems: [{}]", cc.mapped_systems.join(", ")));
            }
            if !cc.associated_symptoms.is_empty() {
                prompt.push_str(&format!(
                    " + associated: [{}]",
                    cc.associated_symptoms.join(", ")
                ));
            }
            prompt.push('\n');
        }
    }
    prompt.push('\n');

    // --- Pertinent negatives ---
    if !session.systems_denied.is_empty() {
        prompt.push_str("PERTINENT NEGATIVES (user denied these systems):\n");
        for system in &session.systems_denied {
            prompt.push_str(&format!("  - {}\n", system));
        }
        prompt.push('\n');
    }

    // --- Current candidates ---
    prompt.push_str("CURRENT CANDIDATES (ranked):\n");
    if session.candidates.is_empty() {
        prompt.push_str("  (no candidates yet — need chief complaint first)\n");
    } else {
        for (i, c) in session.candidates.top(5).iter().enumerate() {
            prompt.push_str(&format!(
                "  {}. {} (score: {:.2}, quality: {:?})",
                i + 1,
                c.ingredient,
                c.composite_score,
                c.quality,
            ));
            if !c.per_symptom_scores.is_empty() {
                let scores: Vec<String> = c
                    .per_symptom_scores
                    .iter()
                    .map(|(s, v)| format!("{}={:.2}", s, v))
                    .collect();
                prompt.push_str(&format!(" [{}]", scores.join(", ")));
            }
            if !c.contraindications.is_empty() {
                prompt.push_str(" ⚠ HAS CONTRAINDICATIONS");
            }
            prompt.push('\n');
        }
    }
    prompt.push('\n');

    // --- Differentiating questions ---
    if !differentiators.is_empty() {
        prompt.push_str("DIFFERENTIATING QUESTIONS AVAILABLE:\n");
        for (i, d) in differentiators.iter().take(3).enumerate() {
            prompt.push_str(&format!(
                "  {}. Topic: \"{}\" (entropy: {:.2})\n     Favors: [{}]\n     Basis: {}\n",
                i + 1,
                d.question_topic,
                d.entropy_score,
                d.favors.join(", "),
                d.graph_basis,
            ));
        }
        prompt.push('\n');
    }

    // --- Systems not yet reviewed ---
    if !unreviewed_systems.is_empty() {
        prompt.push_str("SYSTEMS NOT YET REVIEWED:\n");
        for s in unreviewed_systems {
            prompt.push_str(&format!("  - {}\n", s));
        }
        prompt.push('\n');
    }

    // --- Conversation summary (compressed history) ---
    if let Some(ref summary) = session.turn_summary {
        prompt.push_str("CONVERSATION SUMMARY (earlier turns):\n");
        prompt.push_str(summary);
        prompt.push_str("\n\n");
    }

    // --- Recent conversation turns (so the LLM doesn't repeat itself) ---
    let recent_turns: Vec<_> = session
        .turns
        .iter()
        .rev()
        .take(8) // last 4 exchanges
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    if !recent_turns.is_empty() {
        prompt.push_str("RECENT TURNS (do NOT repeat questions from here):\n");
        for turn in &recent_turns {
            let role = match turn.role {
                TurnRole::User => "User",
                TurnRole::Agent => "You",
            };
            prompt.push_str(&format!("  {}: {}\n", role, turn.text));
        }
        prompt.push('\n');
    }

    // --- Medication check reminder ---
    if !session.asked_about_medications && !session.candidates.is_empty() {
        prompt.push_str(
            "IMPORTANT — YOU HAVE NOT YET ASKED ABOUT MEDICATIONS:\n\
             Before making any recommendations, you MUST ask whether the user\n\
             is currently taking any prescription medications or other supplements.\n\
             This is safety-critical for identifying potential interactions.\n\n",
        );
    }

    // --- Phase-specific task instruction ---
    prompt.push_str("YOUR TASK THIS TURN:\n");
    prompt.push_str(task_instruction(session, differentiators));
    prompt.push('\n');

    // Get the user's latest message
    let user_message = session
        .turns
        .iter()
        .rev()
        .find(|t| t.role == TurnRole::User)
        .map(|t| t.text.clone())
        .unwrap_or_default();

    IntakeContext {
        system_prompt: prompt,
        user_message,
    }
}

// ---------------------------------------------------------------------------
// V2 context builder — driven by intake graph traversal output.
//
// The graph tells us exactly what to ask. The LLM renders it as natural
// conversation. "YOUR TASK THIS TURN" is replaced by the graph's output.
// ---------------------------------------------------------------------------

/// Build a system prompt driven by intake graph traversal results.
///
/// Unlike `build_context` (v1), this does NOT include hardcoded phase
/// instructions or a generic OLDCARTS mnemonic. Instead, the graph
/// traversal provides the specific question to ask, and the executor
/// provides mechanism/interaction/adverse-reaction context.
pub fn build_context_v2(
    session: &IntakeSession,
    turn_action: &TurnAction,
    action_results: &ActionResults,
    relevant_dimensions: &[OldcartsDimension],
) -> IntakeContext {
    let mut prompt = String::with_capacity(4096);

    // --- Role (unchanged) ---
    prompt.push_str(
        "ROLE:\n\
         You are a supplement intake specialist. You gather information about\n\
         a person's symptoms to identify supplements that may help.\n\n",
    );

    // --- Communication style (unchanged) ---
    prompt.push_str(
        "COMMUNICATION STYLE:\n\
         - Keep responses short — a few sentences. No walls of text.\n\
         - Sound like a warm, competent human. Not a textbook.\n\
         - One question per turn. Let the person answer before asking more.\n\
         - No bullet points, no headers, no markdown formatting, no emoji.\n\
         - Use plain language, not medical jargon.\n\
         - For recommendations (final phase only), you may be more thorough.\n\n",
    );

    // --- Legal constraints (unchanged) ---
    prompt.push_str(
        "LEGAL CONSTRAINTS:\n\
         - Never diagnose. Never say \"you have X.\"\n\
         - Never say \"cure.\" Supplements address symptoms, not diseases.\n\
         - Never give instructions like \"take X\" or \"you should try X.\"\n\
           Instead, report what the research suggests: \"For symptoms like\n\
           yours, the literature suggests X may help support...\" You are\n\
           reporting findings, not prescribing.\n\
         - If the user describes an emergency, direct them to call 911.\n\
         - Never recommend specific dosages.\n\n",
    );

    // --- Chief complaints ---
    prompt.push_str("CHIEF COMPLAINTS:\n");
    if session.chief_complaints.is_empty() {
        prompt.push_str("  (none yet — this is the opening question)\n");
    } else {
        for (i, cc) in session.chief_complaints.iter().enumerate() {
            prompt.push_str(&format!("  {}. \"{}\"", i + 1, cc.raw_text));
            if !cc.mapped_symptoms.is_empty() {
                prompt.push_str(&format!(
                    " → symptoms: [{}]",
                    cc.mapped_symptoms.join(", ")
                ));
            }
            if !cc.mapped_systems.is_empty() {
                prompt.push_str(&format!(" → systems: [{}]", cc.mapped_systems.join(", ")));
            }
            prompt.push('\n');
        }
    }
    prompt.push('\n');

    // --- Gathered so far (only relevant dimensions, not all 9) ---
    if !relevant_dimensions.is_empty() {
        prompt.push_str("GATHERED SO FAR (relevant dimensions for this symptom):\n");
        format_relevant_oldcarts(&session.oldcarts, relevant_dimensions, &mut prompt);
        prompt.push('\n');
    }

    // --- Pertinent negatives ---
    if !session.systems_denied.is_empty() {
        prompt.push_str("PERTINENT NEGATIVES:\n");
        for system in &session.systems_denied {
            prompt.push_str(&format!("  - {}\n", system));
        }
        prompt.push('\n');
    }

    // --- Current candidates ---
    if !session.candidates.is_empty() {
        prompt.push_str("CURRENT CANDIDATES (ranked):\n");
        for (i, c) in session.candidates.top(5).iter().enumerate() {
            prompt.push_str(&format!(
                "  {}. {} (score: {:.2}, quality: {:?})\n",
                i + 1,
                c.ingredient,
                c.composite_score,
                c.quality,
            ));
        }
        prompt.push('\n');
    }

    // --- Interaction warnings (from iDISK) ---
    if !action_results.interactions.is_empty() {
        prompt.push_str("⚠ DRUG INTERACTION WARNINGS:\n");
        for ir in &action_results.interactions {
            prompt.push_str(&format!(
                "  {} interacts with {}\n",
                ir.ingredient, ir.drug
            ));
            if let Some(ref desc) = ir.description {
                // Truncate long descriptions
                let short = if desc.len() > 200 {
                    format!("{}...", &desc[..197])
                } else {
                    desc.clone()
                };
                prompt.push_str(&format!("    Reason: {}\n", short));
            }
        }
        prompt.push('\n');
    }

    // --- Adverse reaction matches ---
    if !action_results.adverse_matches.is_empty() {
        prompt.push_str("⚠ POSSIBLE ADVERSE REACTIONS:\n\
                         Some of the user's symptoms may be caused by supplements they are taking:\n");
        for am in &action_results.adverse_matches {
            prompt.push_str(&format!(
                "  {} is a known side effect of {} (source: {})\n",
                am.symptom, am.ingredient, am.source
            ));
        }
        prompt.push_str("  Frame carefully: \"I notice some of your symptoms can be associated with...\"\n\n");
    }

    // --- Mechanism of Action text (for recommendations) ---
    if !action_results.mechanisms.is_empty() {
        prompt.push_str("MECHANISM OF ACTION (sourced — use to explain WHY, not to fabricate):\n");
        for m in &action_results.mechanisms {
            prompt.push_str(&format!("  {}: {}\n", m.ingredient, m.mechanism_text));
        }
        prompt.push('\n');
    }

    // --- Conversation summary ---
    if let Some(ref summary) = session.turn_summary {
        prompt.push_str("CONVERSATION SUMMARY (earlier turns):\n");
        prompt.push_str(summary);
        prompt.push_str("\n\n");
    }

    // --- Recent turns ---
    let recent_turns: Vec<_> = session
        .turns
        .iter()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    if !recent_turns.is_empty() {
        prompt.push_str("RECENT TURNS (do NOT repeat questions from here):\n");
        for turn in &recent_turns {
            let role = match turn.role {
                TurnRole::User => "User",
                TurnRole::Agent => "You",
            };
            prompt.push_str(&format!("  {}: {}\n", role, turn.text));
        }
        prompt.push('\n');
    }

    // --- THE GRAPH-DRIVEN TASK (replaces hardcoded "YOUR TASK THIS TURN") ---
    prompt.push_str("YOUR TASK THIS TURN:\n");

    if let Some(ref question) = turn_action.question {
        // The graph selected a specific question. Tell the LLM to render it.
        prompt.push_str(&format!(
            "Ask this question naturally: \"{}\"\n\
             Rephrase it in your own words to sound conversational.\n\
             Do NOT add extra questions. Just this one.\n",
            question.text
        ));
    } else if let Some(ref next_stage) = turn_action.next_stage {
        match next_stage {
            IntakeStageId::Recommendation => {
                prompt.push_str(
                    "Present what the research suggests for their symptoms.\n\
                     Frame as: \"For symptoms like yours, the literature suggests X\n\
                     may help support...\" — you are REPORTING findings, not prescribing.\n\
                     Never say \"take X\" or \"you should try X.\"\n\
                     For each candidate: which systems it supports and why relevant.\n\
                     Use the MECHANISM OF ACTION text above for sourced explanations.\n\
                     Mention evidence quality and any interaction warnings.\n\
                     End with: \"Please discuss these with your healthcare provider.\"\n",
                );
            }
            IntakeStageId::CausationInquiry => {
                prompt.push_str(
                    "Some of the user's symptoms may be caused by supplements they already take.\n\
                     Gently mention this possibility. Frame as: \"I want to mention —\n\
                     some of your symptoms can be associated with certain supplements.\"\n\
                     Do NOT say \"stop taking X.\" Instead suggest discussing with their doctor.\n",
                );
            }
            _ => {
                prompt.push_str(
                    "Acknowledge what the user said and continue the conversation naturally.\n",
                );
            }
        }
    } else {
        prompt.push_str(
            "Acknowledge what the user said and continue the conversation naturally.\n",
        );
    }

    // --- Traversal trace (debug) ---
    if !turn_action.trace.is_empty() {
        prompt.push_str("\n(Internal trace — do not mention to user: ");
        prompt.push_str(&turn_action.trace.join(" → "));
        prompt.push_str(")\n");
    }

    // Get the user's latest message
    let user_message = session
        .turns
        .iter()
        .rev()
        .find(|t| t.role == TurnRole::User)
        .map(|t| t.text.clone())
        .unwrap_or_default();

    IntakeContext {
        system_prompt: prompt,
        user_message,
    }
}

/// Format only the relevant OLDCARTS dimensions for active symptom profiles.
fn format_relevant_oldcarts(
    oldcarts: &crate::session::OldcartsState,
    relevant: &[OldcartsDimension],
    buf: &mut String,
) {
    for dim in relevant {
        match dim {
            OldcartsDimension::Onset => format_field(buf, "Onset", &oldcarts.onset),
            OldcartsDimension::Location => format_field(buf, "Location", &oldcarts.location),
            OldcartsDimension::Duration => format_field(buf, "Duration", &oldcarts.duration),
            OldcartsDimension::Character => format_field(buf, "Character", &oldcarts.character),
            OldcartsDimension::Aggravating => {
                if oldcarts.aggravating.is_empty() {
                    buf.push_str("  Aggravating: (not yet asked)\n");
                } else {
                    buf.push_str(&format!("  Aggravating: {}\n", oldcarts.aggravating.join(", ")));
                }
            }
            OldcartsDimension::Alleviating => {
                if oldcarts.alleviating.is_empty() {
                    buf.push_str("  Alleviating: (not yet asked)\n");
                } else {
                    buf.push_str(&format!("  Alleviating: {}\n", oldcarts.alleviating.join(", ")));
                }
            }
            OldcartsDimension::Radiation => format_field(buf, "Radiation", &oldcarts.radiation),
            OldcartsDimension::Timing => format_field(buf, "Timing", &oldcarts.timing),
            OldcartsDimension::Severity => {
                match oldcarts.severity {
                    Some(s) => buf.push_str(&format!("  Severity: {}/10\n", s)),
                    None => buf.push_str("  Severity: (not yet asked)\n"),
                }
            }
        }
    }
}

fn format_field(buf: &mut String, label: &str, val: &Option<String>) {
    match val {
        Some(v) => buf.push_str(&format!("  {}: {}\n", label, v)),
        None => buf.push_str(&format!("  {}: (not yet asked)\n", label)),
    }
}

fn phase_description(phase: &IntakePhase) -> &'static str {
    match phase {
        IntakePhase::ChiefComplaint => "Chief Complaint — gather what brings them in today",
        IntakePhase::Hpi => "HPI (OLDCARTS) — deep-dive on each chief complaint",
        IntakePhase::ReviewOfSystems => {
            "Review of Systems — graph-guided system sweep, record pertinent negatives"
        }
        IntakePhase::Differentiation => {
            "Differentiation — ask discriminating questions to narrow candidates"
        }
        IntakePhase::CausationInquiry => {
            "Causation Inquiry — check if user's symptoms may be caused by current supplements"
        }
        IntakePhase::Recommendation => "Recommendation — present final results with reasoning",
    }
}

fn task_instruction(session: &IntakeSession, differentiators: &[Differentiator]) -> &'static str {
    match session.phase {
        IntakePhase::ChiefComplaint => {
            "Ask what brings them in today. If they already stated a complaint,\n\
             acknowledge it and ask if there's anything else."
        }
        IntakePhase::Hpi => {
            "Ask about the next unfilled OLDCARTS dimension that makes sense\n\
             for this complaint. Skip dimensions that clearly don't apply.\n\
             Use clinical judgment:\n\
             - Check RECENT TURNS to see what you've already covered.\n\
             - If the user already answered something implicitly, note it\n\
               and move on rather than re-asking.\n\
             - Most questions you can let go if the user is vague. But for\n\
               safety-critical info (current medications, known conditions),\n\
               it's OK to ask again or explain why it matters.\n\
             - If the user asks why you're asking, explain briefly.\n\
             - If the user goes off-topic, gently redirect."
        }
        IntakePhase::ReviewOfSystems => {
            "Ask about the next unreviewed system relevant to the candidates.\n\
             Frame questions around symptoms, not jargon.\n\
             Example: \"Have you noticed any digestive changes lately?\"\n\
             Accept denials gracefully and move on."
        }
        IntakePhase::Differentiation => {
            if differentiators.is_empty() {
                "No more differentiating questions. Ask if the user is ready\n\
                 to hear what you've found, or has anything else to share."
            } else {
                "Ask the top differentiating question to narrow candidates.\n\
                 Frame it as a natural follow-up. Don't expose graph internals."
            }
        }
        IntakePhase::CausationInquiry => {
            "Some of the user's symptoms may be caused by supplements they already take.\n\
             Gently mention this possibility. Frame as: \"I want to mention —\n\
             some of your symptoms can be associated with certain supplements.\"\n\
             Do NOT say \"stop taking X.\" Instead suggest discussing with their doctor."
        }
        IntakePhase::Recommendation => {
            "Present what the research suggests for their symptoms.\n\
             Frame as: \"For symptoms like yours, the literature suggests X\n\
             may help support...\" — you are REPORTING findings, not prescribing.\n\
             Never say \"take X\" or \"you should try X.\"\n\
             For each candidate: which systems it supports and why relevant.\n\
             Mention evidence quality and any contraindications.\n\
             End with: \"Please discuss these with your healthcare provider.\""
        }
    }
}

fn format_oldcarts(oldcarts: &crate::session::OldcartsState, buf: &mut String) {
    fn field(buf: &mut String, label: &str, val: &Option<String>) {
        match val {
            Some(v) => buf.push_str(&format!("  {}: {}\n", label, v)),
            None => buf.push_str(&format!("  {}: (not yet asked)\n", label)),
        }
    }

    field(buf, "Onset", &oldcarts.onset);
    field(buf, "Location", &oldcarts.location);
    field(buf, "Duration", &oldcarts.duration);
    field(buf, "Character", &oldcarts.character);

    if oldcarts.aggravating.is_empty() {
        buf.push_str("  Aggravating: (not yet asked)\n");
    } else {
        buf.push_str(&format!(
            "  Aggravating: {}\n",
            oldcarts.aggravating.join(", ")
        ));
    }

    if oldcarts.alleviating.is_empty() {
        buf.push_str("  Alleviating: (not yet asked)\n");
    } else {
        buf.push_str(&format!(
            "  Alleviating: {}\n",
            oldcarts.alleviating.join(", ")
        ));
    }

    field(buf, "Radiation", &oldcarts.radiation);
    field(buf, "Timing", &oldcarts.timing);

    match oldcarts.severity {
        Some(s) => buf.push_str(&format!("  Severity: {}/10\n", s)),
        None => buf.push_str("  Severity: (not yet asked)\n"),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::IntakeSession;

    #[test]
    fn test_build_context_initial_session() {
        let session = IntakeSession::new();
        let ctx = build_context(&session, &[], &[]);

        assert!(ctx.system_prompt.contains("ROLE:"));
        assert!(ctx.system_prompt.contains("LEGAL CONSTRAINTS:"));
        assert!(ctx.system_prompt.contains("Chief Complaint"));
        assert!(ctx.system_prompt.contains("none yet"));
        assert!(ctx.system_prompt.contains("OLDCARTS"));
    }

    #[test]
    fn test_build_context_with_complaints() {
        let mut session = IntakeSession::new();
        session.add_complaint("my legs hurt at night");
        session.chief_complaints[0]
            .mapped_symptoms
            .push("muscle cramps".to_string());
        session.phase = IntakePhase::Hpi;

        let ctx = build_context(&session, &[], &[]);
        assert!(ctx.system_prompt.contains("my legs hurt at night"));
        assert!(ctx.system_prompt.contains("muscle cramps"));
        assert!(ctx.system_prompt.contains("HPI"));
    }

    #[test]
    fn test_build_context_with_denied_systems() {
        let mut session = IntakeSession::new();
        session.deny_system("digestive system");

        let ctx = build_context(&session, &[], &[]);
        assert!(ctx.system_prompt.contains("PERTINENT NEGATIVES"));
        assert!(ctx.system_prompt.contains("digestive system"));
    }

    #[test]
    fn test_phase_instructions_vary() {
        let mut session = IntakeSession::new();

        session.phase = IntakePhase::ChiefComplaint;
        let ctx1 = build_context(&session, &[], &[]);

        session.phase = IntakePhase::Recommendation;
        let ctx2 = build_context(&session, &[], &[]);

        assert_ne!(ctx1.system_prompt, ctx2.system_prompt);
        assert!(ctx2.system_prompt.contains("Present what the research suggests"));
    }
}
