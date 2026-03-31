use llm_client::provider::{CompletionRequest, LlmProvider};

// ---------------------------------------------------------------------------
// Symptom resolver — maps free-text symptom phrases to intake profile IDs.
//
// This is a separate LLM call (cheap model, same as extractor) that bridges
// the gap between what users actually say ("jittery", "queasy", "can't think
// straight") and our controlled symptom profile vocabulary.
//
// The profile list is passed in at call time — no hardcoding here. The LLM
// can ONLY return IDs from that list, making it a closed-vocabulary
// classifier rather than a free-form similarity search.
//
// Conservative by design: the prompt instructs the LLM to match only when
// confident, and to respect clinical distinctions (anxiety ≠ depression,
// fatigue ≠ brain fog, bloating ≠ digestive discomfort).
// ---------------------------------------------------------------------------

/// Resolve a list of free-text symptom phrases to known intake profile IDs.
///
/// Returns a deduplicated list of profile IDs from `known_profiles` that
/// the LLM is confident apply. Empty if nothing matches confidently.
pub async fn resolve_symptoms(
    symptom_phrases: &[String],
    known_profiles: &[String],
    extractor: &dyn LlmProvider,
) -> Vec<String> {
    if symptom_phrases.is_empty() || known_profiles.is_empty() {
        return vec![];
    }

    let profile_list = known_profiles.join(", ");
    let phrase_list = symptom_phrases.join("; ");

    let system_prompt = format!(
        r#"You are a clinical symptom classifier. Your job is to map patient-reported symptom phrases to a fixed list of symptom categories.

Known symptom categories (these are the ONLY valid outputs):
{profile_list}

Rules:
- Return ONLY a JSON array of category IDs from the list above.
- Only include a category if you are CONFIDENT it applies.
- Be conservative — do NOT match based on loose similarity alone.
- Respect clinical distinctions: anxiety and depression are different; fatigue and brain_fog are different; bloating and digestive_discomfort are different; back_pain and joint_pain are different.
- A phrase like "jittery" or "on edge" maps to anxiety. "Queasy" maps to nausea. "Can't think straight" maps to brain_fog.
- If nothing matches confidently, return an empty array: []
- Return ONLY the JSON array, no explanation."#
    );

    let user_prompt = format!("Patient reported: {phrase_list}");

    let request = CompletionRequest::new(user_prompt)
        .with_system(system_prompt)
        .with_max_tokens(128)
        .with_temperature(0.0);

    let raw = match extractor.complete(request).await {
        Ok(r) => r.content,
        Err(e) => {
            eprintln!("[symptom_resolver] LLM error: {e}");
            return vec![];
        }
    };

    parse_profile_ids(&raw, known_profiles)
}

/// Parse the LLM's JSON array response and validate against known profiles.
fn parse_profile_ids(raw: &str, known_profiles: &[String]) -> Vec<String> {
    let cleaned = raw
        .trim()
        .strip_prefix("```json")
        .or_else(|| raw.trim().strip_prefix("```"))
        .unwrap_or(raw.trim())
        .strip_suffix("```")
        .unwrap_or(raw.trim())
        .trim();

    let ids: Vec<String> = serde_json::from_str(cleaned).unwrap_or_else(|e| {
        eprintln!("[symptom_resolver] parse error: {e}, raw: {cleaned}");
        vec![]
    });

    // Validate — only keep IDs that are actually in our known list
    let known_set: std::collections::HashSet<&str> =
        known_profiles.iter().map(|s| s.as_str()).collect();

    ids.into_iter()
        .filter(|id| known_set.contains(id.as_str()))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_ids() {
        let known = vec![
            "anxiety".to_string(),
            "nausea".to_string(),
            "fatigue".to_string(),
            "brain_fog".to_string(),
        ];
        let raw = r#"["anxiety", "nausea"]"#;
        let result = parse_profile_ids(raw, &known);
        assert!(result.contains(&"anxiety".to_string()));
        assert!(result.contains(&"nausea".to_string()));
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_rejects_unknown_ids() {
        let known = vec!["anxiety".to_string(), "nausea".to_string()];
        // LLM hallucinated a profile that doesn't exist
        let raw = r#"["anxiety", "vertigo", "cardiac_arrhythmia"]"#;
        let result = parse_profile_ids(raw, &known);
        assert_eq!(result, vec!["anxiety".to_string()]);
    }

    #[test]
    fn test_empty_response() {
        let known = vec!["anxiety".to_string()];
        let raw = "[]";
        let result = parse_profile_ids(raw, &known);
        assert!(result.is_empty());
    }

    #[test]
    fn test_strips_markdown_fences() {
        let known = vec!["fatigue".to_string(), "insomnia".to_string()];
        let raw = "```json\n[\"fatigue\", \"insomnia\"]\n```";
        let result = parse_profile_ids(raw, &known);
        assert_eq!(result.len(), 2);
    }
}
