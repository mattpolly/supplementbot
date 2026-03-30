use axum::extract::{Query, State};
use axum::response::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::state::AppState;

// ---------------------------------------------------------------------------
// Read-only database explorer endpoints
//
// These return raw JSON from the underlying SurrealDB tables.
// All queries are SELECT-only; no mutations are possible via these endpoints.
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct PageQuery {
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    offset: usize,
}

fn default_limit() -> usize {
    50
}

// ---------------------------------------------------------------------------
// Graph explorer endpoints
// ---------------------------------------------------------------------------

/// GET /api/explore/graph/nodes?limit=50&offset=0
pub async fn graph_nodes(
    State(state): State<AppState>,
    Query(page): Query<PageQuery>,
) -> Json<Value> {
    let limit = page.limit.min(200);
    let offset = page.offset;
    let db = state.inner.graph.db();

    let result: Result<Vec<Value>, _> = db
        .query("SELECT id, name, node_type FROM node ORDER BY name LIMIT $limit START $offset")
        .bind(("limit", limit))
        .bind(("offset", offset))
        .await
        .and_then(|mut r| r.take(0));

    match result {
        Ok(rows) => Json(json!({ "rows": rows, "limit": limit, "offset": offset })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// GET /api/explore/graph/edges?limit=50&offset=0
pub async fn graph_edges(
    State(state): State<AppState>,
    Query(page): Query<PageQuery>,
) -> Json<Value> {
    let limit = page.limit.min(200);
    let offset = page.offset;
    let db = state.inner.graph.db();

    let result: Result<Vec<Value>, _> = db
        .query("SELECT *, in AS source, out AS target FROM edge LIMIT $limit START $offset")
        .bind(("limit", limit))
        .bind(("offset", offset))
        .await
        .and_then(|mut r| r.take(0));

    match result {
        Ok(rows) => Json(json!({ "rows": rows, "limit": limit, "offset": offset })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// GET /api/explore/graph/node-aliases?limit=50&offset=0
pub async fn graph_node_aliases(
    State(state): State<AppState>,
    Query(page): Query<PageQuery>,
) -> Json<Value> {
    let limit = page.limit.min(200);
    let offset = page.offset;
    let db = state.inner.graph.db();

    let result: Result<Vec<Value>, _> = db
        .query("SELECT id, canonical, alias, confidence, method FROM node_alias ORDER BY canonical LIMIT $limit START $offset")
        .bind(("limit", limit))
        .bind(("offset", offset))
        .await
        .and_then(|mut r| r.take(0));

    match result {
        Ok(rows) => Json(json!({ "rows": rows, "limit": limit, "offset": offset })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// GET /api/explore/graph/node-cuis?limit=50&offset=0
pub async fn graph_node_cuis(
    State(state): State<AppState>,
    Query(page): Query<PageQuery>,
) -> Json<Value> {
    let limit = page.limit.min(200);
    let offset = page.offset;
    let db = state.inner.graph.db();

    let result: Result<Vec<Value>, _> = db
        .query("SELECT id, node_name, cui, confidence, method FROM node_cui ORDER BY node_name LIMIT $limit START $offset")
        .bind(("limit", limit))
        .bind(("offset", offset))
        .await
        .and_then(|mut r| r.take(0));

    match result {
        Ok(rows) => Json(json!({ "rows": rows, "limit": limit, "offset": offset })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// GET /api/explore/graph/edge-sources?limit=50&offset=0
pub async fn graph_edge_sources(
    State(state): State<AppState>,
    Query(page): Query<PageQuery>,
) -> Json<Value> {
    let limit = page.limit.min(200);
    let offset = page.offset;
    let db = state.inner.graph.db();

    let result: Result<Vec<Value>, _> = db
        .query("SELECT source_node, target_node, edge_type, confidence, source_tag, observation_type, provider, model, observed_at FROM edge_source ORDER BY source_node LIMIT $limit START $offset")
        .bind(("limit", limit))
        .bind(("offset", offset))
        .await
        .and_then(|mut r| r.take(0));

    match result {
        Ok(rows) => Json(json!({ "rows": rows, "limit": limit, "offset": offset })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// GET /api/explore/graph/edge-citations?limit=50&offset=0
pub async fn graph_edge_citations(
    State(state): State<AppState>,
    Query(page): Query<PageQuery>,
) -> Json<Value> {
    let limit = page.limit.min(200);
    let offset = page.offset;
    let db = state.inner.graph.db();

    let result: Result<Vec<Value>, _> = db
        .query("SELECT source_node, target_node, edge_type, pmid, sentence, confidence FROM edge_citation ORDER BY source_node LIMIT $limit START $offset")
        .bind(("limit", limit))
        .bind(("offset", offset))
        .await
        .and_then(|mut r| r.take(0));

    match result {
        Ok(rows) => Json(json!({ "rows": rows, "limit": limit, "offset": offset })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// GET /api/explore/graph/stats — counts for all graph tables
pub async fn graph_stats(State(state): State<AppState>) -> Json<Value> {
    let db = state.inner.graph.db();

    let counts: Vec<(&str, &str)> = vec![
        ("nodes", "SELECT count() FROM node GROUP ALL"),
        ("edges", "SELECT count() FROM edge GROUP ALL"),
        ("node_aliases", "SELECT count() FROM node_alias GROUP ALL"),
        ("node_cuis", "SELECT count() FROM node_cui GROUP ALL"),
        ("edge_sources", "SELECT count() FROM edge_source GROUP ALL"),
        ("edge_citations", "SELECT count() FROM edge_citation GROUP ALL"),
    ];

    let mut stats = serde_json::Map::new();
    for (name, query) in counts {
        let count: Result<Vec<Value>, _> = db
            .query(query)
            .await
            .and_then(|mut r| r.take(0));
        let n = count.ok()
            .and_then(|rows| rows.into_iter().next())
            .and_then(|v| v.get("count").and_then(|c| c.as_u64()))
            .unwrap_or(0);
        stats.insert(name.to_string(), json!(n));
    }

    Json(Value::Object(stats))
}

// ---------------------------------------------------------------------------
// Relational explorer endpoints (intake_ and idisk_ tables)
// ---------------------------------------------------------------------------

/// GET /api/explore/relational/stats — counts for intake + iDISK tables
pub async fn relational_stats(State(state): State<AppState>) -> Json<Value> {
    let db = state.inner.graph.db();

    let counts: Vec<(&str, &str)> = vec![
        ("intake_stages", "SELECT count() FROM intake_stage GROUP ALL"),
        ("intake_archetypes", "SELECT count() FROM intake_archetype GROUP ALL"),
        ("intake_goals", "SELECT count() FROM intake_goal GROUP ALL"),
        ("intake_questions", "SELECT count() FROM intake_question GROUP ALL"),
        ("intake_symptom_profiles", "SELECT count() FROM intake_symptom_profile GROUP ALL"),
        ("intake_exit_conditions", "SELECT count() FROM intake_exit_condition GROUP ALL"),
        ("intake_system_reviews", "SELECT count() FROM intake_system_review GROUP ALL"),
        ("intake_graph_actions", "SELECT count() FROM intake_graph_action GROUP ALL"),
        ("intake_clusters", "SELECT count() FROM intake_cluster GROUP ALL"),
        ("intake_edges", "SELECT count() FROM intake_edge GROUP ALL"),
        ("idisk_drugs", "SELECT count() FROM idisk_drug GROUP ALL"),
        ("idisk_ingredients", "SELECT count() FROM idisk_ingredient GROUP ALL"),
        ("idisk_adverse_reactions", "SELECT count() FROM idisk_adverse_reaction GROUP ALL"),
        ("idisk_interactions", "SELECT count() FROM idisk_interaction GROUP ALL"),
        ("idisk_effectiveness", "SELECT count() FROM idisk_effectiveness GROUP ALL"),
    ];

    let mut stats = serde_json::Map::new();
    for (name, query) in counts {
        let count: Result<Vec<Value>, _> = db
            .query(query)
            .await
            .and_then(|mut r| r.take(0));
        let n = count.ok()
            .and_then(|rows| rows.into_iter().next())
            .and_then(|v| v.get("count").and_then(|c| c.as_u64()))
            .unwrap_or(0);
        stats.insert(name.to_string(), json!(n));
    }

    Json(Value::Object(stats))
}

/// GET /api/explore/relational/intake-stages
pub async fn intake_stages(State(state): State<AppState>) -> Json<Value> {
    let db = state.inner.graph.db();
    let result: Result<Vec<Value>, _> = db
        .query("SELECT * FROM intake_stage ORDER BY id")
        .await
        .and_then(|mut r| r.take(0));
    match result {
        Ok(rows) => Json(json!({ "rows": rows })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// GET /api/explore/relational/intake-archetypes
pub async fn intake_archetypes(State(state): State<AppState>) -> Json<Value> {
    let db = state.inner.graph.db();
    let result: Result<Vec<Value>, _> = db
        .query("SELECT * FROM intake_archetype ORDER BY name")
        .await
        .and_then(|mut r| r.take(0));
    match result {
        Ok(rows) => Json(json!({ "rows": rows })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// GET /api/explore/relational/intake-symptom-profiles?limit=50&offset=0
pub async fn intake_symptom_profiles(
    State(state): State<AppState>,
    Query(page): Query<PageQuery>,
) -> Json<Value> {
    let limit = page.limit.min(200);
    let offset = page.offset;
    let db = state.inner.graph.db();
    let result: Result<Vec<Value>, _> = db
        .query("SELECT * FROM intake_symptom_profile ORDER BY name LIMIT $limit START $offset")
        .bind(("limit", limit))
        .bind(("offset", offset))
        .await
        .and_then(|mut r| r.take(0));
    match result {
        Ok(rows) => Json(json!({ "rows": rows, "limit": limit, "offset": offset })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// GET /api/explore/relational/intake-questions?limit=50&offset=0
pub async fn intake_questions(
    State(state): State<AppState>,
    Query(page): Query<PageQuery>,
) -> Json<Value> {
    let limit = page.limit.min(200);
    let offset = page.offset;
    let db = state.inner.graph.db();
    let result: Result<Vec<Value>, _> = db
        .query("SELECT * FROM intake_question ORDER BY id LIMIT $limit START $offset")
        .bind(("limit", limit))
        .bind(("offset", offset))
        .await
        .and_then(|mut r| r.take(0));
    match result {
        Ok(rows) => Json(json!({ "rows": rows, "limit": limit, "offset": offset })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// GET /api/explore/relational/intake-clusters
pub async fn intake_clusters(State(state): State<AppState>) -> Json<Value> {
    let db = state.inner.graph.db();
    let result: Result<Vec<Value>, _> = db
        .query("SELECT * FROM intake_cluster ORDER BY name")
        .await
        .and_then(|mut r| r.take(0));
    match result {
        Ok(rows) => Json(json!({ "rows": rows })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// GET /api/explore/relational/idisk-ingredients?limit=50&offset=0
pub async fn idisk_ingredients(
    State(state): State<AppState>,
    Query(page): Query<PageQuery>,
) -> Json<Value> {
    let limit = page.limit.min(200);
    let offset = page.offset;
    let db = state.inner.graph.db();
    let result: Result<Vec<Value>, _> = db
        .query("SELECT id, idisk_id, name, cui, safety, background FROM idisk_ingredient ORDER BY name LIMIT $limit START $offset")
        .bind(("limit", limit))
        .bind(("offset", offset))
        .await
        .and_then(|mut r| r.take(0));
    match result {
        Ok(rows) => Json(json!({ "rows": rows, "limit": limit, "offset": offset })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// GET /api/explore/relational/idisk-drugs?limit=50&offset=0
pub async fn idisk_drugs(
    State(state): State<AppState>,
    Query(page): Query<PageQuery>,
) -> Json<Value> {
    let limit = page.limit.min(200);
    let offset = page.offset;
    let db = state.inner.graph.db();
    let result: Result<Vec<Value>, _> = db
        .query("SELECT id, idisk_id, name, cui FROM idisk_drug ORDER BY name LIMIT $limit START $offset")
        .bind(("limit", limit))
        .bind(("offset", offset))
        .await
        .and_then(|mut r| r.take(0));
    match result {
        Ok(rows) => Json(json!({ "rows": rows, "limit": limit, "offset": offset })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// GET /api/explore/relational/idisk-interactions?limit=50&offset=0
pub async fn idisk_interactions(
    State(state): State<AppState>,
    Query(page): Query<PageQuery>,
) -> Json<Value> {
    let limit = page.limit.min(200);
    let offset = page.offset;
    let db = state.inner.graph.db();
    let result: Result<Vec<Value>, _> = db
        .query("SELECT ingredient_id, drug_id, source, interaction_rating, description FROM idisk_interaction ORDER BY ingredient_id LIMIT $limit START $offset")
        .bind(("limit", limit))
        .bind(("offset", offset))
        .await
        .and_then(|mut r| r.take(0));
    match result {
        Ok(rows) => Json(json!({ "rows": rows, "limit": limit, "offset": offset })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// GET /api/explore/relational/idisk-adverse?limit=50&offset=0
pub async fn idisk_adverse(
    State(state): State<AppState>,
    Query(page): Query<PageQuery>,
) -> Json<Value> {
    let limit = page.limit.min(200);
    let offset = page.offset;
    let db = state.inner.graph.db();
    let result: Result<Vec<Value>, _> = db
        .query("SELECT ingredient_id, symptom_id, source FROM idisk_adverse_reaction ORDER BY ingredient_id LIMIT $limit START $offset")
        .bind(("limit", limit))
        .bind(("offset", offset))
        .await
        .and_then(|mut r| r.take(0));
    match result {
        Ok(rows) => Json(json!({ "rows": rows, "limit": limit, "offset": offset })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}
