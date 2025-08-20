use anyhow::Result;
use base64::{Engine as _, engine::general_purpose};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::auth::AuthService;
use crate::config::Settings;
use crate::models::{AuthenticatedClient, WebSocketMessage};
use crate::workflows::DanmakuWorkflow;

pub type ClientSender = mpsc::UnboundedSender<WebSocketMessage>;

#[derive(Clone)]
pub struct WebSocketManager {
    clients: Arc<RwLock<HashMap<Uuid, (AuthenticatedClient, ClientSender)>>>,
    unauthenticated_clients: Arc<RwLock<HashMap<Uuid, ClientSender>>>,
    auth_service: Arc<AuthService>,
    workflow: Arc<DanmakuWorkflow>,
    #[allow(dead_code)]
    settings: Arc<Settings>,
}

impl WebSocketManager {
    pub async fn new(settings: &Settings) -> Result<Self> {
        let auth_service = Arc::new(AuthService::new(settings));
        let workflow = Arc::new(DanmakuWorkflow::new(settings).await?);
        let settings = Arc::new(settings.clone());

        Ok(Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
            unauthenticated_clients: Arc::new(RwLock::new(HashMap::new())),
            auth_service,
            workflow,
            settings,
        })
    }

    pub async fn add_unauthenticated_client(&self, client_id: Uuid, sender: ClientSender) {
        let mut clients = self.unauthenticated_clients.write().await;
        clients.insert(client_id, sender.clone());
        debug!("Added unauthenticated client: {}", client_id);

        // Send welcome message
        let welcome_msg = WebSocketMessage::Connected {
            message: "WebSocket连接已建立，需要认证后才能使用功能".to_string(),
            auth_required: true,
            auth_status: "pending".to_string(),
        };

        if let Err(e) = sender.send(welcome_msg) {
            warn!(
                "Failed to send welcome message to client {}: {}",
                client_id, e
            );
        }
    }

    pub async fn authenticate_client(
        &self,
        client_id: Uuid,
        auth_data: crate::models::AuthData,
    ) -> Result<()> {
        // Authenticate the client
        let authenticated_client = self.auth_service.authenticate(&auth_data)?;
        info!(
            "Client {} authenticated as {}",
            client_id, authenticated_client.user_id
        );

        // Move client from unauthenticated to authenticated
        let sender = {
            let mut unauth_clients = self.unauthenticated_clients.write().await;
            unauth_clients.remove(&client_id)
        };

        if let Some(sender) = sender {
            {
                let mut auth_clients = self.clients.write().await;
                auth_clients.insert(client_id, (authenticated_client.clone(), sender.clone()));
            }

            // Send authentication success message
            let success_msg = WebSocketMessage::AuthSuccess {
                message: "认证成功".to_string(),
                user_id: authenticated_client.user_id,
                auth_type: authenticated_client.auth_type,
            };

            if let Err(e) = sender.send(success_msg) {
                warn!("Failed to send auth success message: {}", e);
            }
        }

        Ok(())
    }

    pub async fn handle_message(&self, client_id: Uuid, message: WebSocketMessage) -> Result<()> {
        match message {
            WebSocketMessage::Auth { auth_data } => {
                if let Err(e) = self.authenticate_client(client_id, auth_data).await {
                    error!("Authentication failed for client {}: {}", client_id, e);
                    self.send_auth_required(client_id).await;
                }
            }
            WebSocketMessage::Danmaku { content, .. } => {
                if !self.is_authenticated(client_id).await {
                    self.send_auth_required(client_id).await;
                    return Ok(());
                }

                self.process_danmaku(client_id, &content).await?;
            }
            WebSocketMessage::Ping => {
                if !self.is_authenticated(client_id).await {
                    self.send_auth_required(client_id).await;
                    return Ok(());
                }

                self.send_to_client(client_id, WebSocketMessage::Pong).await;
            }
            _ => {
                warn!("Unhandled message type from client {}", client_id);
            }
        }

        Ok(())
    }

    async fn process_danmaku(&self, client_id: Uuid, content: &str) -> Result<()> {
        info!("Processing danmaku from client {}: {}", client_id, content);

        // Get user_id from authenticated client
        let user_id = {
            let clients = self.clients.read().await;
            match clients.get(&client_id) {
                Some((authenticated_client, _)) => authenticated_client.user_id.clone(),
                None => {
                    error!("Client {} not found in authenticated clients", client_id);
                    return Err(anyhow::anyhow!("Client not authenticated"));
                }
            }
        };

        // Create progress sender
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();

        // Clone necessary data for the processing task
        let workflow = self.workflow.clone();
        let content_clone = content.to_string();
        let original_content = content.to_string();
        let client_id_for_task = client_id;
        let manager = self.clone();

        // Start progress forwarding task
        let progress_task = tokio::spawn(async move {
            while let Some(progress_msg) = progress_rx.recv().await {
                manager
                    .send_to_client(client_id_for_task, progress_msg)
                    .await;
            }
        });

        // Process danmaku in background
        let processing_task = tokio::spawn(async move {
            workflow
                .process_danmaku(&content_clone, &user_id, Some(progress_tx))
                .await
        });

        // Wait for processing to complete
        let result = processing_task.await??;

        // Stop progress forwarding
        progress_task.abort();

        // Send final result
        let result_msg = WebSocketMessage::DanmakuResult {
            success: true,
            original_danmaku: original_content,
            intent_type: result.intent_type.as_str().to_string(),
            text_response: result.text_response,
            has_audio: result.audio_data.is_some(),
            has_image: result.image_url.is_some(),
            audio_data: result
                .audio_data
                .map(|data| general_purpose::STANDARD.encode(data)),
            image_data: result.image_url,
        };

        self.send_to_client(client_id, result_msg).await;

        Ok(())
    }

    async fn is_authenticated(&self, client_id: Uuid) -> bool {
        let clients = self.clients.read().await;
        clients.contains_key(&client_id)
    }

    async fn send_auth_required(&self, client_id: Uuid) {
        let auth_required_msg = WebSocketMessage::AuthRequired {
            message: "使用任何功能都需要先进行身份认证".to_string(),
        };

        // Try authenticated clients first
        if self
            .send_to_client(client_id, auth_required_msg.clone())
            .await
        {
            return;
        }

        // Try unauthenticated clients
        let unauth_clients = self.unauthenticated_clients.read().await;
        if let Some(sender) = unauth_clients.get(&client_id) {
            if let Err(e) = sender.send(auth_required_msg) {
                warn!("Failed to send auth required message: {}", e);
            }
        }
    }

    async fn send_to_client(&self, client_id: Uuid, message: WebSocketMessage) -> bool {
        let clients = self.clients.read().await;
        if let Some((_, sender)) = clients.get(&client_id) {
            if let Err(e) = sender.send(message) {
                warn!("Failed to send message to client {}: {}", client_id, e);
                return false;
            }
            return true;
        }
        false
    }

    pub async fn send_to_client_direct(&self, client_id: Uuid, message: WebSocketMessage) {
        // First try authenticated clients
        if self.send_to_client(client_id, message.clone()).await {
            return;
        }

        // Then try unauthenticated clients
        let unauth_clients: tokio::sync::RwLockReadGuard<
            '_,
            HashMap<Uuid, mpsc::UnboundedSender<WebSocketMessage>>,
        > = self.unauthenticated_clients.read().await;
        if let Some(sender) = unauth_clients.get(&client_id) {
            if let Err(e) = sender.send(message) {
                warn!(
                    "Failed to send message to unauthenticated client {}: {}",
                    client_id, e
                );
            }
        }
    }

    pub async fn remove_client(&self, client_id: Uuid) {
        {
            let mut clients = self.clients.write().await;
            if clients.remove(&client_id).is_some() {
                info!("Removed authenticated client: {}", client_id);
                return;
            }
        }

        {
            let mut unauth_clients = self.unauthenticated_clients.write().await;
            if unauth_clients.remove(&client_id).is_some() {
                info!("Removed unauthenticated client: {}", client_id);
            }
        }
    }

    #[allow(dead_code)]
    pub async fn broadcast_to_authenticated(&self, message: WebSocketMessage) {
        let clients = self.clients.read().await;
        let mut failed_clients = Vec::new();

        for (client_id, (_, sender)) in clients.iter() {
            if let Err(_) = sender.send(message.clone()) {
                failed_clients.push(*client_id);
            }
        }

        // Clean up failed clients
        if !failed_clients.is_empty() {
            drop(clients);
            let mut clients_write = self.clients.write().await;
            for client_id in failed_clients {
                clients_write.remove(&client_id);
                warn!("Removed failed client during broadcast: {}", client_id);
            }
        }
    }

    pub async fn get_authenticated_client_count(&self) -> usize {
        let clients = self.clients.read().await;
        clients.len()
    }

    pub async fn get_total_client_count(&self) -> usize {
        let auth_count = self.get_authenticated_client_count().await;
        let unauth_clients = self.unauthenticated_clients.read().await;
        auth_count + unauth_clients.len()
    }
}
