use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::handler::{self, TurnResult};
use crate::state::AppState;

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
}

impl ServerMessage {
    fn welcome(session_id: &Uuid) -> Self {
        Self {
            msg_type: "welcome".to_string(),
            session_id: Some(session_id.to_string()),
            text: None,
            phase: Some("chief_complaint".to_string()),
            message: None,
            complete: None,
            candidate_count: None,
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
        }
    }
}

/// Axum handler — upgrades HTTP to WebSocket.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> Response {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

/// Handle one WebSocket connection.
async fn handle_socket(mut socket: WebSocket, state: AppState) {
    // Try to create a session
    let session_id = match state.inner.sessions.create_session().await {
        Ok(id) => id,
        Err(denied) => {
            let msg = ServerMessage::denied(&denied.to_string());
            let _ = send_json(&mut socket, &msg).await;
            return;
        }
    };

    eprintln!("[ws] session {session_id} started");

    // Send welcome + generate opening message
    let _ = send_json(&mut socket, &ServerMessage::welcome(&session_id)).await;

    // Generate the opening prompt (no user message yet — renderer says hello)
    let _ = send_json(&mut socket, &ServerMessage::typing()).await;
    if let Some(opening) = generate_opening(&state, &session_id).await {
        let _ = send_json(&mut socket, &ServerMessage::from_turn(&opening)).await;
    }

    // Main message loop
    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Message::Text(text) => {
                let client_msg: ClientMessage = match serde_json::from_str(&text) {
                    Ok(m) => m,
                    Err(_) => {
                        let _ = send_json(
                            &mut socket,
                            &ServerMessage::error("invalid JSON"),
                        )
                        .await;
                        continue;
                    }
                };

                if client_msg.msg_type != "message" || client_msg.text.is_empty() {
                    continue;
                }

                // Send typing indicator
                let _ = send_json(&mut socket, &ServerMessage::typing()).await;

                // Process the turn
                match handler::process_turn(&state, &session_id, &client_msg.text).await {
                    Some(result) => {
                        let complete = result.complete;
                        let _ = send_json(&mut socket, &ServerMessage::from_turn(&result)).await;
                        if complete {
                            break;
                        }
                    }
                    None => {
                        let _ = send_json(
                            &mut socket,
                            &ServerMessage::error("session expired"),
                        )
                        .await;
                        break;
                    }
                }
            }
            Message::Close(_) => break,
            _ => {} // ignore ping/pong/binary
        }
    }

    eprintln!("[ws] session {session_id} ended");
}

/// Generate the opening "What brings you in today?" message.
async fn generate_opening(state: &AppState, session_id: &Uuid) -> Option<TurnResult> {
    let s = &state.inner;

    let intake_context = s.sessions.with_session(session_id, |session| {
        intake_agent::context::build_context(session, &[], &[])
    }).await?;

    let request = llm_client::provider::CompletionRequest::new("(Patient has just connected. Greet them briefly and ask what brings them in today. Two sentences max.)")
        .with_system(intake_context.system_prompt)
        .with_max_tokens(100)
        .with_temperature(0.7);

    let response = match s.renderer.complete(request).await {
        Ok(resp) => resp.content,
        Err(e) => {
            eprintln!("[session {session_id}] opening error: {e}");
            "Hello! Welcome. What brings you in today?".to_string()
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
    })
}

async fn send_json(socket: &mut WebSocket, msg: &ServerMessage) -> Result<(), axum::Error> {
    let text = serde_json::to_string(msg).unwrap();
    socket.send(Message::Text(text.into())).await
}
