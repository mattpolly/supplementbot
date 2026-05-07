use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Public types — unchanged from the SurrealDB version so callers don't break.
// ---------------------------------------------------------------------------

/// A PubMed citation backing a specific edge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CitationRecord {
    pub source_node: String,
    pub target_node: String,
    pub edge_type: String,
    pub pmid: String,
    pub sentence: String,
    pub confidence: f64,
    pub suppkg_predicate: String,
    pub source_cui: String,
    pub target_cui: String,
}

/// A single observation of an edge by a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeObservation {
    pub source_node: String,
    pub target_node: String,
    pub edge_type: String,
    pub confidence: f64,
    pub source_tag: String,
    pub observation_type: String,
    pub provider: String,
    pub model: String,
    pub observed_at: String,
    pub correlation_id: String,
}

/// A single observation of a node by a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeObservation {
    pub node_name: String,
    pub node_type: String,
    pub provider: String,
    pub model: String,
    pub observed_at: String,
    pub correlation_id: String,
}

/// Per-provider observation summary for an edge.
#[derive(Debug, Clone)]
pub struct ProviderObservation {
    pub provider: String,
    pub model: String,
    pub observation_type: String,
    pub confidence: f64,
}

/// Provider agreement summary for a specific edge.
#[derive(Debug, Clone)]
pub struct EdgeAgreement {
    pub provider_count: usize,
    pub providers: Vec<ProviderObservation>,
    pub total_observations: usize,
}

/// An edge annotated with quality metadata.
#[derive(Debug, Clone)]
pub struct EdgeWithQuality {
    pub source_node: String,
    pub target_node: String,
    pub edge_type: String,
    pub quality: EdgeQuality,
    pub provider_count: usize,
    pub total_observations: usize,
    pub has_citation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EdgeQuality {
    /// Edge exists in graph but has no NSAI observations (legacy/seed data).
    Deduced,
    Speculative,
    SingleProvider,
    MultiProvider,
    CitationBacked,
}

impl EdgeQuality {
    pub fn label(&self) -> &'static str {
        match self {
            EdgeQuality::Deduced => "deduced",
            EdgeQuality::Speculative => "speculative",
            EdgeQuality::SingleProvider => "single_provider",
            EdgeQuality::MultiProvider => "multi_provider",
            EdgeQuality::CitationBacked => "citation_backed",
        }
    }
}

/// An edge confirmed by multiple LLM providers.
#[derive(Debug, Clone)]
pub struct MultiProviderEdge {
    pub source_node: String,
    pub target_node: String,
    pub edge_type: String,
    pub providers: Vec<String>,
}

// ---------------------------------------------------------------------------
// SourceStore — Postgres-backed projection of NSAI provenance data.
// ---------------------------------------------------------------------------

pub struct SourceStore {
    pool: PgPool,
}

impl SourceStore {
    pub fn new(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
    }

    // -- Write operations --------------------------------------------------

    pub async fn record_node_observation(
        &self,
        node_name: &str,
        _node_type: &str,
        provider: &str,
        model: &str,
        correlation_id: Uuid,
        observed_at: DateTime<Utc>,
    ) {
        // Resolve entity by name
        let entity_id: Option<Uuid> = sqlx::query_scalar!(
            "SELECT e.id FROM entity e WHERE LOWER(e.name) = LOWER($1) LIMIT 1",
            node_name,
        )
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();

        if let Some(entity_id) = entity_id {
            let _ = sqlx::query!(
                r#"
                INSERT INTO node_observation (entity_id, provider, model, correlation_id, observed_at)
                VALUES ($1, $2, $3, $4, $5)
                ON CONFLICT DO NOTHING
                "#,
                entity_id, provider, model, correlation_id, observed_at,
            )
            .execute(&self.pool)
            .await;
        }
    }

    pub async fn record_edge_created(
        &self,
        source_node: &str,
        target_node: &str,
        edge_type: &str,
        confidence: f64,
        source_tag: &str,
        provider: &str,
        model: &str,
        correlation_id: Uuid,
        observed_at: DateTime<Utc>,
    ) {
        self.record_edge_observation(
            source_node, target_node, edge_type,
            confidence, source_tag, "created",
            provider, model, correlation_id, observed_at,
        ).await;
    }

    pub async fn record_edge_confirmed(
        &self,
        source_node: &str,
        target_node: &str,
        edge_type: &str,
        provider: &str,
        model: &str,
        correlation_id: Uuid,
        observed_at: DateTime<Utc>,
    ) {
        self.record_edge_observation(
            source_node, target_node, edge_type,
            0.0, "Confirmed", "confirmed",
            provider, model, correlation_id, observed_at,
        ).await;
    }

    async fn record_edge_observation(
        &self,
        source_node: &str,
        target_node: &str,
        edge_type: &str,
        confidence: f64,
        source_tag: &str,
        observation_type: &str,
        provider: &str,
        model: &str,
        correlation_id: Uuid,
        observed_at: DateTime<Utc>,
    ) {
        let rel_id = self.resolve_relationship(source_node, target_node, edge_type).await;
        if let Some(rel_id) = rel_id {
            let _ = sqlx::query!(
                r#"
                INSERT INTO edge_observation
                    (relationship_id, observation_type, provider, model,
                     confidence, source_tag, correlation_id, observed_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                "#,
                rel_id, observation_type, provider, model,
                confidence as f32, source_tag, correlation_id, observed_at,
            )
            .execute(&self.pool)
            .await;
        }
    }

    async fn resolve_relationship(
        &self,
        source_node: &str,
        target_node: &str,
        edge_type: &str,
    ) -> Option<Uuid> {
        let pg_edge_type = normalize_edge_type(edge_type);
        sqlx::query_scalar!(
            r#"
            SELECT r.id FROM relationship r
            JOIN rel_type rt ON rt.id = r.rel_type_id
            JOIN entity e1 ON e1.id = r.from_entity
            JOIN entity e2 ON e2.id = r.to_entity
            WHERE LOWER(e1.name) = LOWER($1)
              AND LOWER(e2.name) = LOWER($2)
              AND rt.name = $3
            LIMIT 1
            "#,
            source_node, target_node, pg_edge_type,
        )
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    // -- Read operations --------------------------------------------------

    pub async fn observations_for_edge(
        &self,
        source_node: &str,
        target_node: &str,
        edge_type: &str,
    ) -> Vec<EdgeObservation> {
        let rel_id = self.resolve_relationship(source_node, target_node, edge_type).await;
        let Some(rel_id) = rel_id else { return vec![] };

        sqlx::query!(
            r#"
            SELECT eo.observation_type, eo.provider, eo.model,
                   eo.confidence, eo.source_tag,
                   eo.correlation_id, eo.observed_at
            FROM edge_observation eo
            WHERE eo.relationship_id = $1
            ORDER BY eo.observed_at ASC
            "#,
            rel_id,
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| EdgeObservation {
            source_node: source_node.to_string(),
            target_node: target_node.to_string(),
            edge_type: edge_type.to_string(),
            confidence: r.confidence as f64,
            source_tag: r.source_tag,
            observation_type: r.observation_type,
            provider: r.provider,
            model: r.model,
            observed_at: r.observed_at.to_rfc3339(),
            correlation_id: r.correlation_id
                .map(|u| u.to_string())
                .unwrap_or_default(),
        })
        .collect()
    }

    pub async fn provider_agreement(
        &self,
        source_node: &str,
        target_node: &str,
        edge_type: &str,
    ) -> EdgeAgreement {
        let observations = self.observations_for_edge(source_node, target_node, edge_type).await;
        let providers: Vec<ProviderObservation> = observations
            .iter()
            .map(|o| ProviderObservation {
                provider: o.provider.clone(),
                model: o.model.clone(),
                observation_type: o.observation_type.clone(),
                confidence: o.confidence,
            })
            .collect();
        let unique: std::collections::HashSet<&str> =
            providers.iter().map(|p| p.provider.as_str()).collect();
        EdgeAgreement {
            provider_count: unique.len(),
            total_observations: observations.len(),
            providers,
        }
    }

    pub async fn total_edge_observations(&self) -> usize {
        sqlx::query_scalar!("SELECT COUNT(*) FROM edge_observation")
            .fetch_one(&self.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(0) as usize
    }

    pub async fn total_node_observations(&self) -> usize {
        sqlx::query_scalar!("SELECT COUNT(*) FROM node_observation")
            .fetch_one(&self.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(0) as usize
    }

    pub async fn edges_by_quality(&self) -> Vec<EdgeWithQuality> {
        let rows = sqlx::query!(
            r#"
            SELECT
                e1.name AS source_node,
                e2.name AS target_node,
                rt.name AS edge_type,
                COUNT(DISTINCT eo.provider) AS provider_count,
                COUNT(eo.id) AS total_obs,
                BOOL_OR(ec.id IS NOT NULL) AS has_citation
            FROM relationship r
            JOIN rel_type rt ON rt.id = r.rel_type_id
            JOIN entity e1 ON e1.id = r.from_entity
            JOIN entity e2 ON e2.id = r.to_entity
            LEFT JOIN edge_observation eo ON eo.relationship_id = r.id
            LEFT JOIN evidence_claim ec ON ec.entity_id = r.from_entity
                AND ec.source = 'supplementbot_confirmed'
                AND ec.attrs->>'target_node' = LOWER(e2.name)
            WHERE r.source LIKE 'nsai%'
            GROUP BY r.id, e1.name, e2.name, rt.name
            ORDER BY provider_count DESC, total_obs DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        rows.into_iter()
            .map(|r| {
                let provider_count = r.provider_count.unwrap_or(0) as usize;
                let total_obs = r.total_obs.unwrap_or(0) as usize;
                let has_citation = r.has_citation.unwrap_or(false);
                let quality = if has_citation {
                    EdgeQuality::CitationBacked
                } else if provider_count >= 2 {
                    EdgeQuality::MultiProvider
                } else if provider_count == 1 {
                    EdgeQuality::SingleProvider
                } else {
                    EdgeQuality::Speculative
                };
                EdgeWithQuality {
                    source_node: r.source_node,
                    target_node: r.target_node,
                    edge_type: r.edge_type,
                    quality,
                    provider_count,
                    total_observations: total_obs,
                    has_citation,
                }
            })
            .collect()
    }

    pub async fn edges_at_quality(&self, min_quality: EdgeQuality) -> Vec<EdgeWithQuality> {
        self.edges_by_quality()
            .await
            .into_iter()
            .filter(|e| e.quality >= min_quality)
            .collect()
    }

    pub async fn multi_provider_edges(&self) -> Vec<MultiProviderEdge> {
        let rows = sqlx::query!(
            r#"
            SELECT
                e1.name AS source_node,
                e2.name AS target_node,
                rt.name AS edge_type,
                ARRAY_AGG(DISTINCT eo.provider) AS providers
            FROM relationship r
            JOIN rel_type rt ON rt.id = r.rel_type_id
            JOIN entity e1 ON e1.id = r.from_entity
            JOIN entity e2 ON e2.id = r.to_entity
            JOIN edge_observation eo ON eo.relationship_id = r.id
            WHERE r.source LIKE 'nsai%'
            GROUP BY r.id, e1.name, e2.name, rt.name
            HAVING COUNT(DISTINCT eo.provider) >= 2
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        rows.into_iter()
            .map(|r| MultiProviderEdge {
                source_node: r.source_node,
                target_node: r.target_node,
                edge_type: r.edge_type,
                providers: r.providers.unwrap_or_default(),
            })
            .collect()
    }

    // -- Citation methods -------------------------------------------------

    pub async fn record_citation(&self, citation: &CitationRecord) -> bool {
        if citation.pmid.is_empty() || citation.pmid == "0" {
            return false;
        }

        // Resolve entity
        let entity_id: Option<Uuid> = sqlx::query_scalar!(
            r#"
            SELECT e.id FROM synonym s JOIN entity e ON e.id = s.entity_id
            WHERE LOWER(s.name) = LOWER($1) AND e.source = 'seed' LIMIT 1
            "#,
            citation.source_node,
        )
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();

        let Some(entity_id) = entity_id else { return false };

        // Upsert citation
        let citation_id: Option<Uuid> = sqlx::query_scalar!(
            "SELECT id FROM citation WHERE pmid = $1",
            citation.pmid,
        )
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();

        let citation_id = if let Some(id) = citation_id {
            id
        } else {
            sqlx::query_scalar!(
                r#"
                INSERT INTO citation (pmid, abstract_text, source)
                VALUES ($1, $2, 'supplementbot_confirmed')
                RETURNING id
                "#,
                citation.pmid, citation.sentence,
            )
            .fetch_one(&self.pool)
            .await
            .unwrap_or(Uuid::new_v4())
        };

        // Insert evidence_claim
        let exists: bool = sqlx::query_scalar!(
            "SELECT EXISTS(SELECT 1 FROM evidence_claim WHERE entity_id = $1 AND citation_id = $2 AND source = 'supplementbot_confirmed')",
            entity_id, citation_id,
        )
        .fetch_one(&self.pool)
        .await
        .ok()
        .flatten()
        .unwrap_or(false);

        if exists {
            return false;
        }

        let direction = direction_for_predicate(&citation.suppkg_predicate);
        let attrs = serde_json::json!({
            "edge_type": citation.edge_type,
            "suppkg_predicate": citation.suppkg_predicate,
            "target_node": citation.target_node,
            "target_cui": citation.target_cui,
            "source_cui": citation.source_cui,
        });

        let _ = sqlx::query!(
            r#"
            INSERT INTO evidence_claim
                (entity_id, citation_id, claim_type, claim_text, direction, confidence, source, attrs)
            VALUES ($1, $2, 'graph_edge', $3, $4, $5, 'supplementbot_confirmed', $6)
            "#,
            entity_id, citation_id, citation.sentence,
            direction, citation.confidence as f32,
            attrs,
        )
        .execute(&self.pool)
        .await;

        true
    }

    pub async fn existing_pmids_for(&self, source_node: &str) -> std::collections::HashSet<String> {
        sqlx::query_scalar!(
            r#"
            SELECT c.pmid FROM citation c
            JOIN evidence_claim ec ON ec.citation_id = c.id
            JOIN entity e ON e.id = ec.entity_id
            WHERE LOWER(e.name) = LOWER($1)
              AND ec.source = 'supplementbot_confirmed'
              AND c.pmid IS NOT NULL
            "#,
            source_node,
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect()
    }

    pub async fn record_citations_batch(&self, citations: &[CitationRecord]) -> usize {
        if citations.is_empty() {
            return 0;
        }
        let source_node = &citations[0].source_node;
        let existing = self.existing_pmids_for(source_node).await;
        let mut stored = 0;
        for citation in citations {
            if existing.contains(&citation.pmid) {
                continue;
            }
            if self.record_citation(citation).await {
                stored += 1;
            }
        }
        stored
    }

    pub async fn citations_for_edge(
        &self,
        source_node: &str,
        target_node: &str,
        _edge_type: &str,
    ) -> Vec<CitationRecord> {
        sqlx::query!(
            r#"
            SELECT c.pmid, ec.claim_text AS sentence, ec.confidence,
                   ec.attrs->>'edge_type' AS edge_type,
                   ec.attrs->>'suppkg_predicate' AS suppkg_predicate,
                   ec.attrs->>'target_node' AS target_node_attr,
                   ec.attrs->>'target_cui' AS target_cui,
                   ec.attrs->>'source_cui' AS source_cui
            FROM evidence_claim ec
            JOIN citation c ON c.id = ec.citation_id
            JOIN entity e ON e.id = ec.entity_id
            WHERE LOWER(e.name) = LOWER($1)
              AND LOWER(ec.attrs->>'target_node') = LOWER($2)
              AND ec.source = 'supplementbot_confirmed'
            ORDER BY ec.confidence DESC
            "#,
            source_node, target_node,
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| CitationRecord {
            source_node: source_node.to_string(),
            target_node: r.target_node_attr.unwrap_or_else(|| target_node.to_string()),
            edge_type: r.edge_type.unwrap_or_default(),
            pmid: r.pmid.unwrap_or_default(),
            sentence: r.sentence,
            confidence: r.confidence as f64,
            suppkg_predicate: r.suppkg_predicate.unwrap_or_default(),
            source_cui: r.source_cui.unwrap_or_default(),
            target_cui: r.target_cui.unwrap_or_default(),
        })
        .collect()
    }

    pub async fn citations_for_ingredient(&self, ingredient: &str) -> Vec<CitationRecord> {
        sqlx::query!(
            r#"
            SELECT c.pmid, ec.claim_text AS sentence, ec.confidence,
                   ec.attrs->>'edge_type' AS edge_type,
                   ec.attrs->>'suppkg_predicate' AS suppkg_predicate,
                   ec.attrs->>'target_node' AS target_node,
                   ec.attrs->>'target_cui' AS target_cui,
                   ec.attrs->>'source_cui' AS source_cui
            FROM evidence_claim ec
            JOIN citation c ON c.id = ec.citation_id
            JOIN entity e ON e.id = ec.entity_id
            JOIN entity_type et ON et.id = e.type_id
            WHERE LOWER(e.name) = LOWER($1)
              AND ec.source = 'supplementbot_confirmed'
            ORDER BY ec.confidence DESC
            LIMIT 100
            "#,
            ingredient,
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| CitationRecord {
            source_node: ingredient.to_string(),
            target_node: r.target_node.unwrap_or_default(),
            edge_type: r.edge_type.unwrap_or_default(),
            pmid: r.pmid.unwrap_or_default(),
            sentence: r.sentence,
            confidence: r.confidence as f64,
            suppkg_predicate: r.suppkg_predicate.unwrap_or_default(),
            source_cui: r.source_cui.unwrap_or_default(),
            target_cui: r.target_cui.unwrap_or_default(),
        })
        .collect()
    }

    pub async fn all_citations(&self) -> Vec<CitationRecord> {
        // Intentionally limited — this is a bulk dump used for debugging only
        self.citations_for_ingredient("").await
    }

    pub async fn citation_count(&self) -> usize {
        sqlx::query_scalar!(
            "SELECT COUNT(*) FROM evidence_claim WHERE source = 'supplementbot_confirmed'"
        )
        .fetch_one(&self.pool)
        .await
        .ok()
        .flatten()
        .unwrap_or(0) as usize
    }

    pub async fn cited_edge_count(&self) -> usize {
        sqlx::query_scalar!(
            r#"
            SELECT COUNT(DISTINCT (entity_id, attrs->>'target_node'))
            FROM evidence_claim WHERE source = 'supplementbot_confirmed'
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .ok()
        .flatten()
        .unwrap_or(0) as usize
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn normalize_edge_type(s: &str) -> String {
    match s.to_lowercase().as_str() {
        "actson" | "acts_on" => "acts_on".to_string(),
        "affords" => "affords".to_string(),
        "viamechanism" | "via_mechanism" => "via_mechanism".to_string(),
        "presentsin" | "presents_in" => "presents_in".to_string(),
        "modulates" => "modulates".to_string(),
        "contraindicatedwith" | "contraindicated_with" => "contraindicated_with".to_string(),
        "competeswith" | "competes_with" => "competes_with".to_string(),
        "disinhibits" => "disinhibits".to_string(),
        "sequesters" => "sequesters".to_string(),
        "releases" => "releases".to_string(),
        "amplifies" => "amplifies".to_string(),
        "desensitizes" => "desensitizes".to_string(),
        "positivelyreinforces" | "positively_reinforces" => "positively_reinforces".to_string(),
        "gates" => "gates".to_string(),
        other => other.to_string(),
    }
}

fn direction_for_predicate(predicate: &str) -> &'static str {
    match predicate.to_uppercase().as_str() {
        "TREATS" | "PREVENTS" | "STIMULATES" | "AUGMENTS" | "PRODUCES" => "positive",
        "INHIBITS" | "DISRUPTS" | "CAUSES" | "PREDISPOSES" => "negative",
        "supplementology" | "affords" => "positive",
        _ => "neutral",
    }
}
