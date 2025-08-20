use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::mpsc;
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::config::Settings;
use crate::models::WebSocketMessage;
use crate::services::WebSocketManager;

pub struct WebSocketServer {
    settings: Settings,
    manager: Arc<WebSocketManager>,
}

impl WebSocketServer {
    pub fn new(settings: Settings) -> Self {
        let manager = Arc::new(WebSocketManager::new(&settings));

        Self { settings, manager }
    }

    pub async fn start(self) -> Result<()> {
        let app = Router::new()
            .route("/", get(root_handler))
            .route("/health", get(health_handler))
            .route("/ws", get(websocket_handler))
            .route("/stats", get(stats_handler))
            .layer(
                ServiceBuilder::new()
                    .layer(TraceLayer::new_for_http())
                    .layer(CorsLayer::permissive()),
            )
            .with_state(self.manager);

        let addr = format!("{}:{}", self.settings.server.host, self.settings.server.port);
        info!("Starting server on {}", addr);

        let listener = tokio::net::TcpListener::bind(&addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }
}

async fn root_handler() -> impl IntoResponse {
    "VTuber Rig WebSocket Server is running!"
}

async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "healthy",
        "service": "vtuber-rig",
        "version": "0.1.0"
    }))
}

async fn stats_handler(State(manager): State<Arc<WebSocketManager>>) -> impl IntoResponse {
    let authenticated_count = manager.get_authenticated_client_count().await;
    let total_count = manager.get_total_client_count().await;

    Json(serde_json::json!({
        "authenticated_clients": authenticated_count,
        "total_clients": total_count,
        "unauthenticated_clients": total_count - authenticated_count
    }))
}

async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(manager): State<Arc<WebSocketManager>>,
) -> Response {
    ws.on_upgrade(|socket| handle_websocket(socket, manager))
}

async fn handle_websocket(socket: WebSocket, manager: Arc<WebSocketManager>) {
    let client_id = Uuid::new_v4();
    info!("New WebSocket connection: {}", client_id);

    let (ws_sender, mut ws_receiver) = socket.split();
    let (tx, rx) = mpsc::unbounded_channel::<WebSocketMessage>();

    // Add client to manager
    manager.add_unauthenticated_client(client_id, tx).await;

    // Spawn task to handle outgoing messages
    let client_id_for_sender = client_id;
    let sender_task = tokio::spawn(async move {
        let mut sender = ws_sender;
        let mut rx = rx;
        while let Some(msg) = rx.recv().await {
            match serde_json::to_string(&msg) {
                Ok(json_str) => {
                    if let Err(e) = sender.send(Message::Text(json_str.into())).await {
                        error!("Failed to send message to client {}: {}", client_id_for_sender, e);
                        break;
                    }
                }
                Err(e) => {
                    error!("Failed to serialize message: {}", e);
                }
            }
        }
    });

    // Handle incoming messages
    let client_id_for_receiver = client_id;
    let manager_for_receiver = manager.clone();
    while let Some(msg_result) = ws_receiver.next().await {
        match msg_result {
            Ok(Message::Text(text)) => {
                match serde_json::from_str::<WebSocketMessage>(&text) {
                    Ok(message) => {
                        if let Err(e) = manager_for_receiver.handle_message(client_id_for_receiver, message).await {
                            error!("Error handling message from client {}: {}", client_id_for_receiver, e);
                            
                            // Send error message via manager
                            let error_msg = WebSocketMessage::Error {
                                message: format!("处理消息时发生错误: {}", e),
                            };
                            
                            manager_for_receiver.send_to_client_direct(client_id_for_receiver, error_msg).await;
                        }
                    }
                    Err(e) => {
                        warn!("Failed to parse message from client {}: {}", client_id_for_receiver, e);
                        
                        let error_msg = WebSocketMessage::Error {
                            message: "消息格式无效".to_string(),
                        };
                        
                        manager_for_receiver.send_to_client_direct(client_id_for_receiver, error_msg).await;
                    }
                }
            }
            Ok(Message::Close(_)) => {
                info!("Client {} disconnected", client_id_for_receiver);
                break;
            }
            Ok(Message::Ping(_data)) => {
                // Note: We can't directly send pong here due to ownership
                // This should be handled via the message system if needed
                warn!("Received ping from {}, but cannot respond directly", client_id_for_receiver);
            }
            Ok(_) => {
                // Handle other message types if needed
            }
            Err(e) => {
                error!("WebSocket error for client {}: {}", client_id_for_receiver, e);
                break;
            }
        }
    }

    // Clean up
    sender_task.abort();
    manager.remove_client(client_id).await;
    info!("Client {} disconnected and cleaned up", client_id);
}
