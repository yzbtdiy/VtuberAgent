use std::{convert::Infallible, net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    extract::{Query, State},
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
    Json, Router,
};
use futures_util::stream::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::BroadcastStream;
use tower_http::cors::CorsLayer;

use crate::errors::{AgentError, Result};

pub type BroadcastSender = broadcast::Sender<String>;

#[derive(Clone)]
pub struct SignatureAuth {
    access_key: String,
    secret_key: String,
    max_age: Duration,
}

impl SignatureAuth {
    pub fn new(access_key: String, secret_key: String, max_age: Duration) -> Self {
        Self {
            access_key,
            secret_key,
            max_age,
        }
    }

    pub fn verify_params(&self, params: &AuthParams) -> bool {
        if params.access_key != self.access_key {
            return false;
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let diff = now.abs_diff(params.timestamp);
        if diff > self.max_age.as_secs() {
            return false;
        }

        let canonical = format!("{}:{}:{}", params.access_key, params.timestamp, params.nonce);
        let signature_bytes = match hex::decode(&params.signature) {
            Ok(bytes) => bytes,
            Err(_) => return false,
        };

        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        let mut mac = match Hmac::<Sha256>::new_from_slice(self.secret_key.as_bytes()) {
            Ok(mac) => mac,
            Err(_) => return false,
        };
        mac.update(canonical.as_bytes());
        mac.verify_slice(&signature_bytes).is_ok()
    }
}

#[derive(Debug, Deserialize)]
pub struct AuthParams {
    access_key: String,
    timestamp: i64,
    nonce: String,
    signature: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ClientMessage {
    Command { input: String },
    LiveStart,
    LiveStop,
    LiveStatus,
}

#[derive(Debug)]
pub enum AgentCommand {
    Command { input: String },
    LiveStart,
    LiveStop,
    LiveStatus,
}

impl From<ClientMessage> for AgentCommand {
    fn from(value: ClientMessage) -> Self {
        match value {
            ClientMessage::Command { input } => AgentCommand::Command { input },
            ClientMessage::LiveStart => AgentCommand::LiveStart,
            ClientMessage::LiveStop => AgentCommand::LiveStop,
            ClientMessage::LiveStatus => AgentCommand::LiveStatus,
        }
    }
}

pub fn message_bus() -> (BroadcastSender, broadcast::Receiver<String>) {
    broadcast::channel(256)
}

pub fn encode_message(event: &str, payload: Value) -> String {
    json!({
        "event": event,
        "payload": payload,
    })
    .to_string()
}

pub fn broadcast_json(sender: &BroadcastSender, event: &str, payload: Value) {
    let message = encode_message(event, payload);
    let _ = sender.send(message);
}

#[derive(Clone)]
struct AppState {
    auth: Arc<SignatureAuth>,
    broadcaster: BroadcastSender,
    command_tx: mpsc::Sender<AgentCommand>,
}

pub async fn run_server(
    addr: SocketAddr,
    auth: Arc<SignatureAuth>,
    broadcaster: BroadcastSender,
    command_tx: mpsc::Sender<AgentCommand>,
) -> Result<()> {
    let state = AppState {
        auth,
        broadcaster,
        command_tx,
    };

    let app = Router::new()
        .route("/events", get(sse_handler))
        .route("/command", post(command_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(target: "sse", %addr, "SSE 服务器已启动");

    axum::serve(listener, app)
        .await
        .map_err(|err| AgentError::other(format!("SSE 服务器错误: {err}")))?;

    Ok(())
}

async fn sse_handler(
    Query(params): Query<AuthParams>,
    State(state): State<AppState>,
) -> std::result::Result<Sse<impl Stream<Item = std::result::Result<Event, Infallible>>>, axum::http::StatusCode>
{
    if !state.auth.verify_params(&params) {
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    }

    let rx = state.broadcaster.subscribe();
    let stream = BroadcastStream::new(rx)
        .filter_map(|result| async move {
            match result {
                Ok(message) => {
                    let event = Event::default().data(message);
                    Some(Ok(event))
                }
                Err(_) => None,
            }
        });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

async fn command_handler(
    Query(params): Query<AuthParams>,
    State(state): State<AppState>,
    Json(message): Json<ClientMessage>,
) -> std::result::Result<Json<Value>, axum::http::StatusCode> {
    if !state.auth.verify_params(&params) {
        return Err(axum::http::StatusCode::UNAUTHORIZED);
    }

    if state.command_tx.send(message.into()).await.is_err() {
        return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE);
    }

    Ok(Json(json!({ "status": "accepted" })))
}
