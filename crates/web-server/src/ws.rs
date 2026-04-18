use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::handler::{self, CitationRef, TurnResult};
use crate::state::AppState;

#[derive(Deserialize, Default)]
pub struct WsQuery {
    #[serde(default)]
    donor: bool,
}

// ---------------------------------------------------------------------------
// WebSocket handler — one connection per patient session.
//
// Protocol (JSON over WebSocket):
//   Client → Server: { "type": "message", "text": "..." }
//   Server → Client: { "type": "response", ... TurnResult fields }
//   Server → Client: { "type": "welcome", "session_id": "..." }
//   Server → Client: { "type": "error", "message": "..." }
//   Server → Client: { "type": "typing" }  (before LLM call)
//   Server → Client: { "type": "emergency" }
//   Server → Client: { "type": "denied", "message": "..." }
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ClientMessage {
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(default)]
    text: String,
}

#[derive(Serialize)]
struct ServerMessage {
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    complete: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    candidate_count: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    citations: Vec<CitationRef>,
}

impl ServerMessage {
    fn ready() -> Self {
        Self {
            msg_type: "ready".to_string(),
            session_id: None,
            text: None,
            phase: None,
            message: None,
            complete: None,
            candidate_count: None,
            citations: vec![],
        }
    }

    fn welcome(session_id: &Uuid) -> Self {
        Self {
            msg_type: "welcome".to_string(),
            session_id: Some(session_id.to_string()),
            text: None,
            phase: Some("chief_complaint".to_string()),
            message: None,
            complete: None,
            candidate_count: None,
            citations: vec![],
        }
    }

    fn typing() -> Self {
        Self {
            msg_type: "typing".to_string(),
            session_id: None,
            text: None,
            phase: None,
            message: None,
            complete: None,
            candidate_count: None,
            citations: vec![],
        }
    }

    fn from_turn(result: &TurnResult) -> Self {
        if result.emergency {
            return Self {
                msg_type: "emergency".to_string(),
                session_id: None,
                text: None,
                phase: None,
                message: None,
                complete: Some(true),
                candidate_count: None,
                citations: vec![],
            };
        }
        Self {
            msg_type: "response".to_string(),
            session_id: None,
            text: Some(result.response.clone()),
            phase: Some(result.phase.clone()),
            message: None,
            complete: Some(result.complete),
            candidate_count: Some(result.candidate_count),
            citations: result.citations.clone(),
        }
    }

    fn denied(reason: &str) -> Self {
        Self {
            msg_type: "denied".to_string(),
            session_id: None,
            text: None,
            phase: None,
            message: Some(reason.to_string()),
            complete: None,
            candidate_count: None,
            citations: vec![],
        }
    }

    fn error(msg: &str) -> Self {
        Self {
            msg_type: "error".to_string(),
            session_id: None,
            text: None,
            phase: None,
            message: Some(msg.to_string()),
            complete: None,
            candidate_count: None,
            citations: vec![],
        }
    }
}

/// Axum handler — upgrades HTTP to WebSocket.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQuery>,
    State(state): State<AppState>,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state, query.donor))
}

/// Handle one WebSocket connection.
///
/// Session is created on connect so the opening greeting fires immediately.
/// The denied check happens at connect time — capacity-limited connections
/// are rejected before the user types anything.
async fn handle_socket(mut socket: WebSocket, state: AppState, donor: bool) {
    // Create the session immediately so we can send the opening greeting.
    let session_id = match state.inner.sessions.create_session(donor).await {
        Ok(id) => id,
        Err(denied) => {
            let _ = send_json(&mut socket, &ServerMessage::denied(&denied.to_string())).await;
            return;
        }
    };

    eprintln!("[ws] session {session_id} started");

    // Send welcome (gives the frontend the session_id) then the opening greeting.
    let _ = send_json(&mut socket, &ServerMessage::welcome(&session_id)).await;
    let _ = send_json(&mut socket, &ServerMessage::typing()).await;
    let opening = generate_opening(&state, &session_id).await;
    let opening_result = opening.unwrap_or_else(|| TurnResult {
        response: "Hello! Welcome to SupplementBot — I'm currently under construction, but I'd love to try to help. What brings you in today?".to_string(),
        phase: "chief_complaint".to_string(),
        emergency: false,
        complete: false,
        candidate_count: 0,
        citations: vec![],
    });
    let _ = send_json(&mut socket, &ServerMessage::from_turn(&opening_result)).await;

    // Main message loop
    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Message::Text(text) => {
                let client_msg: ClientMessage = match serde_json::from_str(&text) {
                    Ok(m) => m,
                    Err(_) => {
                        let _ = send_json(&mut socket, &ServerMessage::error("invalid JSON")).await;
                        continue;
                    }
                };

                if client_msg.msg_type != "message" || client_msg.text.is_empty() {
                    continue;
                }

                let sid = session_id;

                // Send typing indicator
                let _ = send_json(&mut socket, &ServerMessage::typing()).await;

                // Intercept "what supplements do you know about?" before the intake handler.
                if is_asking_about_known_supplements(&client_msg.text) {
                    let ingredients = state.inner.graph.known_ingredients().await;
                    let reply = if ingredients.is_empty() {
                        "I haven't been trained on any specific supplements yet — my knowledge graph is still being built.".to_string()
                    } else {
                        let list = ingredients.join(", ");
                        format!(
                            "I've done research on {} supplement{}: {}. What symptoms or concerns can I help you with today?",
                            ingredients.len(),
                            if ingredients.len() == 1 { "" } else { "s" },
                            list
                        )
                    };
                    let result = TurnResult {
                        response: reply,
                        phase: "chief_complaint".to_string(),
                        emergency: false,
                        complete: false,
                        candidate_count: 0,
                        citations: vec![],
                    };
                    let _ = send_json(&mut socket, &ServerMessage::from_turn(&result)).await;
                    continue;
                }

                // Process the turn
                match handler::process_turn(&state, &sid, &client_msg.text).await {
                    Some(result) => {
                        let complete = result.complete;
                        let _ = send_json(&mut socket, &ServerMessage::from_turn(&result)).await;
                        if complete {
                            break;
                        }
                    }
                    None => {
                        let _ = send_json(&mut socket, &ServerMessage::error("session expired")).await;
                        break;
                    }
                }
            }
            Message::Close(_) => break,
            _ => {} // ignore ping/pong/binary
        }
    }

    state.inner.sessions.remove_session(&session_id).await;
    eprintln!("[ws] session {session_id} ended");
}

/// Generate the opening "What brings you in today?" message.
async fn generate_opening(state: &AppState, session_id: &Uuid) -> Option<TurnResult> {
    let s = &state.inner;

    let intake_context = s.sessions.with_session(session_id, |session| {
        intake_agent::context::build_context(session, &[], &[])
    }).await?;

    // Build a brief coverage hint for the opening prompt
    let coverage_hint = {
        use graph_service::query::CoverageStrength;
        let strong: Vec<&str> = s.archetype_coverage
            .iter()
            .filter(|c| c.strength == CoverageStrength::Strong)
            .map(|c| c.archetype_name.as_str())
            .collect();
        if strong.is_empty() {
            String::new()
        } else {
            format!(
                " My research is strongest in: {}.",
                strong.join(", ")
            )
        }
    };

    let prompt = format!(
        "(Patient has just connected. Greet them warmly. Mention that you are currently under construction but would love to try to help.{} Then ask what brings them in today. Three sentences max.)",
        coverage_hint
    );

    let request = llm_client::provider::CompletionRequest::new(&prompt)
        .with_system(intake_context.system_prompt)
        .with_max_tokens(120)
        .with_temperature(0.7);

    let response = match s.renderer.complete(request).await {
        Ok(resp) => resp.content,
        Err(e) => {
            eprintln!("[session {session_id}] opening error: {e}");
            "Hello! Welcome to SupplementBot — I'm currently under construction, but I'd love to try to help. What brings you in today?".to_string()
        }
    };

    let safe_response = match s.safety_filter.check(&response) {
        intake_agent::safety::FilterResult::Pass(text) => text,
        _ => "Hello! Welcome. What brings you in today?".to_string(),
    };

    s.sessions.with_session(session_id, |session| {
        session.add_agent_turn(&safe_response);
    }).await;

    Some(TurnResult {
        response: safe_response,
        phase: "chief_complaint".to_string(),
        emergency: false,
        complete: false,
        candidate_count: 0,
        citations: vec![],
    })
}

async fn send_json(socket: &mut WebSocket, msg: &ServerMessage) -> Result<(), axum::Error> {
    let text = serde_json::to_string(msg).unwrap();
    socket.send(Message::Text(text.into())).await
}

/// Returns true if the user's message is asking what supplements the bot knows about.
fn is_asking_about_known_supplements(text: &str) -> bool {
    let lower = text.to_lowercase();
    let supplement_words = ["supplement", "supplements", "ingredient", "ingredients", "nutraceutical"];
    let knowledge_words = ["know", "studied", "research", "trained", "have data", "cover", "support", "list"];
    let has_supplement = supplement_words.iter().any(|w| lower.contains(w));
    let has_knowledge = knowledge_words.iter().any(|w| lower.contains(w));
    has_supplement && has_knowledge
}
