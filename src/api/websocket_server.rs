use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::{WebSocketStream, accept_async, tungstenite::Message};
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
    pub async fn new(settings: Settings) -> Result<Self> {
        let manager = Arc::new(WebSocketManager::new(&settings).await?);

        Ok(Self { settings, manager })
    }

    pub async fn start(self) -> Result<()> {
        let addr = format!(
            "{}:{}",
            self.settings.server.host, self.settings.server.port
        );
        info!("Starting WebSocket server on {}", addr);

        let listener = TcpListener::bind(&addr).await?;

        while let Ok((stream, addr)) = listener.accept().await {
            let manager = Arc::clone(&self.manager);
            tokio::spawn(handle_connection(stream, addr, manager));
        }

        Ok(())
    }
}

async fn handle_connection(stream: TcpStream, addr: SocketAddr, manager: Arc<WebSocketManager>) {
    info!("New connection from: {}", addr);

    let ws_stream = match accept_async(stream).await {
        Ok(ws_stream) => ws_stream,
        Err(e) => {
            error!("Failed to accept WebSocket connection from {}: {}", addr, e);
            return;
        }
    };

    let client_id = Uuid::new_v4();
    info!("WebSocket connection established: {} ({})", client_id, addr);

    handle_websocket(ws_stream, client_id, manager).await;
}

async fn handle_websocket(
    socket: WebSocketStream<TcpStream>,
    client_id: Uuid,
    manager: Arc<WebSocketManager>,
) {
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
                        error!(
                            "Failed to send message to client {}: {}",
                            client_id_for_sender, e
                        );
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
                        if let Err(e) = manager_for_receiver
                            .handle_message(client_id_for_receiver, message)
                            .await
                        {
                            error!(
                                "Error handling message from client {}: {}",
                                client_id_for_receiver, e
                            );

                            // Send error message via manager
                            let error_msg = WebSocketMessage::Error {
                                message: format!("处理消息时发生错误: {}", e),
                            };

                            manager_for_receiver
                                .send_to_client_direct(client_id_for_receiver, error_msg)
                                .await;
                        }
                    }
                    Err(e) => {
                        warn!(
                            "Failed to parse message from client {}: {}",
                            client_id_for_receiver, e
                        );

                        let error_msg = WebSocketMessage::Error {
                            message: "消息格式无效".to_string(),
                        };

                        manager_for_receiver
                            .send_to_client_direct(client_id_for_receiver, error_msg)
                            .await;
                    }
                }
            }
            Ok(Message::Close(_)) => {
                info!("Client {} disconnected", client_id_for_receiver);
                break;
            }
            Ok(Message::Ping(_data)) => {
                // Send pong response - for now just log it
                info!("Received ping from client {}", client_id_for_receiver);
            }
            Ok(Message::Pong(_)) => {
                info!("Received pong from client {}", client_id_for_receiver);
            }
            Ok(Message::Binary(_)) => {
                warn!(
                    "Received binary message from client {}, ignoring",
                    client_id_for_receiver
                );
            }
            Ok(Message::Frame(_)) => {
                // Handle raw frames if needed
                info!("Received raw frame from client {}", client_id_for_receiver);
            }
            Err(e) => {
                error!(
                    "WebSocket error for client {}: {}",
                    client_id_for_receiver, e
                );
                break;
            }
        }
    }

    // Clean up
    sender_task.abort();
    manager.remove_client(client_id).await;
    info!("Client {} disconnected and cleaned up", client_id);
}
