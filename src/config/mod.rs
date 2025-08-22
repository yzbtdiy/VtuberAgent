use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub server: ServerConfig,
    pub openai: OpenAIConfig,
    pub indextts: IndexTTSConfig,
    pub dmeta_embedding: DmetaEmbeddingConfig,
    pub auth: AuthConfig,
    pub processing: ProcessingConfig,
    pub long_term_memory: LongTermMemoryConfig,
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub tts_provider: String, // "openai" or "indextts"
    pub embedding_provider: String, // "rust_hash", "openai", or "dmeta-embedding-zh"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub embedding_model: String,
    pub tts_model: String,
    pub tts_voice: String, // New field for TTS voice
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexTTSConfig {
    pub url: String,
    pub model: String,
    pub voice: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmetaEmbeddingConfig {
    pub url: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub secret_key: String,
    pub valid_api_keys: Vec<String>,
    pub timestamp_tolerance: u64, // seconds
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessingConfig {
    pub max_danmaku_length: usize,
    pub response_timeout: u64,   // seconds
    pub max_execution_time: u64, // seconds
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub rust_log: String,
    pub otel_sdk_disabled: bool,
    pub crewai_telemetry_disabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LongTermMemoryConfig {
    pub enabled: bool,
    pub qdrant: QdrantConfig,
    pub context: ContextConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QdrantConfig {
    pub url: String,
    pub api_key: Option<String>,
    pub collection_name: String,
    pub vector_size: u64,
    pub distance: String,
    pub enable_embedding_provider_filter: bool, // 是否启用按嵌入提供商过滤
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextConfig {
    pub max_context_length: usize,
    pub similarity_threshold: f32,
    pub memory_retention_days: u32,
}

impl Settings {
    pub fn load() -> Result<Self> {
        let config_path = Path::new("config.json");

        if !config_path.exists() {
            return Err(anyhow::anyhow!("config.json file not found"));
        }

        let content = fs::read_to_string(config_path).context("Failed to read config.json file")?;

        let settings: Settings =
            serde_json::from_str(&content).context("Failed to parse config.json file")?;

        Ok(settings)
    }
}
