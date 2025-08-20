use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use qdrant_client::{
    Payload, Qdrant,
    qdrant::{
        CreateCollectionBuilder, Datatype, Distance, PointStruct, UpsertPointsBuilder, VectorParams,
    },
};
use serde_json::json;
use tracing::info;

use crate::config::{LongTermMemoryConfig, QdrantConfig};

pub struct LongTermMemory {
    qdrant_client: Option<Qdrant>,
    config: LongTermMemoryConfig,
}

#[derive(Debug, Clone)]
pub struct MemoryItem {
    pub id: String,
    pub user_id: String,
    pub content: String,
    pub intent: String,
    pub timestamp: DateTime<Utc>,
    pub context: Option<String>,
}

impl LongTermMemory {
    pub async fn new(config: LongTermMemoryConfig) -> Result<Self> {
        if !config.enabled {
            info!("Long-term memory is disabled");
            return Ok(Self {
                qdrant_client: None,
                config,
            });
        }

        let qdrant_client = Self::setup_qdrant_client(&config.qdrant).await?;

        Ok(Self {
            qdrant_client: Some(qdrant_client),
            config,
        })
    }

    async fn setup_qdrant_client(qdrant_config: &QdrantConfig) -> Result<Qdrant> {
        // Initialize Qdrant client
        let mut qdrant_builder = Qdrant::from_url(&qdrant_config.url);

        // Add API key if provided
        if let Some(api_key) = &qdrant_config.api_key {
            qdrant_builder = qdrant_builder.api_key(api_key.clone());
        }

        let qdrant_client = qdrant_builder
            .build()
            .context("Failed to connect to Qdrant")?;

        // Create collection if it doesn't exist
        let distance = match qdrant_config.distance.as_str() {
            "Cosine" => Distance::Cosine,
            "Dot" => Distance::Dot,
            "Euclid" => Distance::Euclid,
            "Manhattan" => Distance::Manhattan,
            _ => Distance::Cosine,
        };

        let collection_exists = qdrant_client
            .collection_exists(&qdrant_config.collection_name)
            .await
            .unwrap_or(false);

        if !collection_exists {
            info!(
                "Creating Qdrant collection: {}",
                qdrant_config.collection_name
            );

            let vector_params = VectorParams {
                size: qdrant_config.vector_size,
                distance: distance.into(),
                hnsw_config: None,
                quantization_config: None,
                on_disk: None,
                datatype: Some(Datatype::Float32.into()),
                multivector_config: None,
            };

            qdrant_client
                .create_collection(
                    CreateCollectionBuilder::new(&qdrant_config.collection_name)
                        .vectors_config(vector_params),
                )
                .await
                .context("Failed to create Qdrant collection")?;
        }

        Ok(qdrant_client)
    }

    pub async fn store_interaction(
        &self,
        memory_item: MemoryItem,
        embeddings: Vec<f32>,
    ) -> Result<()> {
        if !self.config.enabled || self.qdrant_client.is_none() {
            return Ok(());
        }

        let qdrant_client = self.qdrant_client.as_ref().unwrap();

        // Create a comprehensive document for storage
        let document_content = format!(
            "User: {} | Intent: {} | Content: {} | Context: {} | Time: {}",
            memory_item.user_id,
            memory_item.intent,
            memory_item.content,
            memory_item.context.as_deref().unwrap_or(""),
            memory_item.timestamp.format("%Y-%m-%d %H:%M:%S")
        );

        let point = PointStruct::new(
            memory_item.id.clone(),
            embeddings,
            Payload::try_from(json!({
                "user_id": memory_item.user_id,
                "content": memory_item.content,
                "intent": memory_item.intent,
                "context": memory_item.context,
                "timestamp": memory_item.timestamp.to_rfc3339(),
                "document": document_content,
            }))
            .unwrap(),
        );

        // Store in Qdrant
        qdrant_client
            .upsert_points(UpsertPointsBuilder::new(
                &self.config.qdrant.collection_name,
                vec![point],
            ))
            .await
            .context("Failed to store memory in Qdrant")?;

        info!("Stored interaction in long-term memory: {}", memory_item.id);
        Ok(())
    }

    pub async fn retrieve_relevant_context(
        &self,
        query_embeddings: Vec<f32>,
        user_id: &str,
    ) -> Result<Vec<String>> {
        if !self.config.enabled || self.qdrant_client.is_none() {
            return Ok(vec![]);
        }

        let qdrant_client = self.qdrant_client.as_ref().unwrap();

        // Build search query
        use qdrant_client::qdrant::SearchPointsBuilder;

        let search_request = SearchPointsBuilder::new(
            &self.config.qdrant.collection_name,
            query_embeddings,
            self.config.context.max_context_length as u64,
        )
        .with_payload(true)
        .build();

        let response = qdrant_client
            .search_points(search_request)
            .await
            .context("Failed to search long-term memory")?;

        let mut context_items = Vec::new();

        for point in response.result {
            // Check similarity threshold
            if point.score >= self.config.context.similarity_threshold {
                let payload = &point.payload;
                if let Some(user_id_value) = payload.get("user_id") {
                    if let Some(stored_user_id) = user_id_value.as_str() {
                        if stored_user_id == user_id {
                            if let Some(content_value) = payload.get("content") {
                                if let Some(content) = content_value.as_str() {
                                    context_items.push(content.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        if !context_items.is_empty() {
            info!(
                "Retrieved {} relevant context items for user {}",
                context_items.len(),
                user_id
            );
        }

        Ok(context_items)
    }

    #[allow(dead_code)]
    pub async fn cleanup_old_memories(&self) -> Result<()> {
        if !self.config.enabled || self.qdrant_client.is_none() {
            return Ok(());
        }

        // This is a placeholder for cleanup logic
        // In a real implementation, you would query by timestamp and delete old entries
        info!("Memory cleanup is not implemented yet");
        Ok(())
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }
}

impl Default for LongTermMemory {
    fn default() -> Self {
        use crate::config::{ContextConfig, QdrantConfig};

        let config = LongTermMemoryConfig {
            enabled: false,
            qdrant: QdrantConfig {
                url: "http://localhost:6334".to_string(),
                api_key: None,
                collection_name: "vtuber_memory".to_string(),
                vector_size: 1536,
                distance: "Cosine".to_string(),
            },
            context: ContextConfig {
                max_context_length: 10,
                similarity_threshold: 0.7,
                memory_retention_days: 30,
            },
        };

        Self {
            qdrant_client: None,
            config,
        }
    }
}
