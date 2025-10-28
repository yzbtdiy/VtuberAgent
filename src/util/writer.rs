use std::path::PathBuf;

use crate::{
    capabilities::BinaryArtifact,
    errors::Result,
    intent::Intent,
    util::{beijing_rfc3339, format_beijing, now_in_beijing},
};
use serde_json::{Map, Value, json};
use tokio::fs;
use uuid::Uuid;

pub struct ArtifactWriter {
    root: PathBuf,
}

impl ArtifactWriter {
    pub async fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root).await?;
        Ok(Self { root })
    }

    pub async fn persist(&self, intent: Intent, artifact: &BinaryArtifact) -> Result<PathBuf> {
        fs::create_dir_all(&self.root).await?;
        let now = now_in_beijing();
        let timestamp = format_beijing(&now, "%Y%m%d_%H%M%S");
        let id = Uuid::new_v4();
        let base_name = format!(
            "{}_{}_{}",
            intent.as_prefix(),
            timestamp,
            &id.to_string()[..8]
        );

        let file_name = format!("{}.{}", base_name, artifact.file_extension);
        let file_path = self.root.join(&file_name);
        fs::write(&file_path, &artifact.data).await?;

        let mut meta = Map::new();
        meta.insert("intent".to_string(), json!(intent.to_string()));
        meta.insert("media_type".to_string(), json!(artifact.media_type));
        meta.insert("description".to_string(), json!(artifact.summary));
        meta.insert("artifact".to_string(), json!(file_name));
        meta.insert("created_at".to_string(), json!(beijing_rfc3339(&now)));

        if let Some(prompt) = artifact
            .metadata
            .get("prompt")
            .and_then(|value| value.as_str())
        {
            meta.insert("prompt".to_string(), Value::String(prompt.to_string()));
        }

        if !artifact.metadata.is_empty() {
            meta.insert(
                "metadata".to_string(),
                Value::Object(artifact.metadata.clone()),
            );
        }

        let meta_value = Value::Object(meta);

        let meta_path = self.root.join(format!("{}.meta.json", base_name));
        fs::write(&meta_path, serde_json::to_vec_pretty(&meta_value)?).await?;

        Ok(file_path)
    }
}
