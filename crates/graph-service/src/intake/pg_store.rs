// ---------------------------------------------------------------------------
// PgIntakeStore — HTTP-backed intake knowledge store.
//
// Loads all intake config from supplementology's Postgres-backed API at
// startup and caches it in memory. The data is static (seeded by Alembic)
// so a one-time load is sufficient. Individual lookups are O(n) over a
// small fixed dataset (<30 rows per table).
// ---------------------------------------------------------------------------

use super::types::*;
use crate::client::SupplementClient;

pub struct PgIntakeStore {
    archetypes: Vec<ArchetypeProfile>,
    profiles: Vec<SymptomProfile>,
    questions: Vec<QuestionTemplate>,
    clusters: Vec<SymptomCluster>,
}

impl PgIntakeStore {
    /// Construct directly from pre-built data (used in tests).
    pub fn from_parts(
        archetypes: Vec<ArchetypeProfile>,
        profiles: Vec<SymptomProfile>,
        questions: Vec<QuestionTemplate>,
        clusters: Vec<SymptomCluster>,
    ) -> Self {
        Self { archetypes, profiles, questions, clusters }
    }

    pub async fn load(client: &SupplementClient) -> Self {
        let archetypes = client.intake_archetypes().await;
        let profiles = client.intake_symptom_profiles().await;
        let questions = client.intake_questions().await;
        let clusters = client.intake_clusters().await;
        eprintln!(
            "  intake config: {} archetypes, {} profiles, {} questions, {} clusters",
            archetypes.len(), profiles.len(), questions.len(), clusters.len()
        );
        Self { archetypes, profiles, questions, clusters }
    }

    // -- Archetypes -----------------------------------------------------------

    pub fn all_archetypes(&self) -> &[ArchetypeProfile] {
        &self.archetypes
    }

    pub fn get_archetype(&self, id: &str) -> Option<&ArchetypeProfile> {
        self.archetypes.iter().find(|a| a.id == id)
    }

    // -- Symptom profiles -----------------------------------------------------

    pub fn get_symptom_profile(&self, id: &str) -> Option<&SymptomProfile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn all_symptom_profile_ids(&self) -> Vec<String> {
        self.profiles.iter().map(|p| p.id.clone()).collect()
    }

    /// Find a symptom profile by name or alias (case-insensitive).
    /// Tiers: exact ID → exact name/alias → substring.
    pub fn find_symptom_profile(&self, name: &str) -> Option<&SymptomProfile> {
        let lower = name.to_lowercase();
        let trimmed = lower.trim();

        // Tier 1: exact ID
        if let Some(p) = self.profiles.iter().find(|p| p.id == trimmed) {
            return Some(p);
        }

        // Tier 2: exact name or alias
        for p in &self.profiles {
            if p.name.to_lowercase() == trimmed {
                return Some(p);
            }
            if p.aliases.iter().any(|a| a.to_lowercase() == trimmed) {
                return Some(p);
            }
        }

        // Tier 3: substring — prefer longest match
        let mut best: Option<(&SymptomProfile, usize)> = None;
        for p in &self.profiles {
            let name_lower = p.name.to_lowercase();
            if trimmed.contains(&name_lower) || name_lower.contains(trimmed) {
                let len = name_lower.len();
                if best.map_or(true, |(_, l)| len > l) {
                    best = Some((p, len));
                }
            }
            for alias in &p.aliases {
                let alias_lower = alias.to_lowercase();
                if trimmed.contains(&alias_lower) || alias_lower.contains(trimmed) {
                    let len = alias_lower.len();
                    if best.map_or(true, |(_, l)| len > l) {
                        best = Some((p, len));
                    }
                }
            }
        }
        best.map(|(p, _)| p)
    }

    // -- Questions ------------------------------------------------------------

    pub fn get_question(&self, id: &str) -> Option<&QuestionTemplate> {
        self.questions.iter().find(|q| q.id == id)
    }

    pub fn all_questions(&self) -> &[QuestionTemplate] {
        &self.questions
    }

    /// Get all questions associated with a body system (system review questions).
    pub fn questions_for_system(&self, system_name: &str) -> Vec<&QuestionTemplate> {
        let lower = system_name.to_lowercase();
        self.questions.iter()
            .filter(|q| q.system_name.as_ref().map_or(false, |s| s.to_lowercase() == lower))
            .collect()
    }

    // -- Clusters -------------------------------------------------------------

    pub fn all_clusters(&self) -> &[SymptomCluster] {
        &self.clusters
    }

    pub fn clusters_for_symptom(&self, symptom_id: &str) -> Vec<&SymptomCluster> {
        self.clusters.iter()
            .filter(|c| c.member_symptoms.iter().any(|m| m == symptom_id))
            .collect()
    }

    pub fn question_count(&self) -> usize {
        self.questions.len()
    }

    pub fn cluster_count(&self) -> usize {
        self.clusters.len()
    }
}
