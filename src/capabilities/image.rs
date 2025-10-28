use crate::{capabilities::BinaryArtifact, errors::Result};
use rig::{
    client::ImageGenerationClient, image_generation::ImageGenerationModel, providers::openai,
};
use serde_json::{Map, Value, json};

pub struct ImageGenerator {
    model: openai::image_generation::ImageGenerationModel,
    model_name: String,
    default_width: u32,
    default_height: u32,
}

impl ImageGenerator {
    pub fn new(client: openai::Client, model_name: &str) -> Self {
        let model = client.image_generation_model(model_name);
        Self {
            model,
            model_name: model_name.to_string(),
            default_width: 1024,
            default_height: 1024,
        }
    }

    pub async fn generate(
        &self,
        prompt: &str,
        resolution: Option<(u32, u32)>,
    ) -> Result<BinaryArtifact> {
        let (width, height) = resolution.unwrap_or((self.default_width, self.default_height));
        let response = self
            .model
            .image_generation_request()
            .prompt(prompt)
            .width(width)
            .height(height)
            .send()
            .await?;

        let mut metadata = Map::new();
        metadata.insert("prompt".to_string(), Value::String(prompt.to_string()));
        metadata.insert("model".to_string(), Value::String(self.model_name.clone()));
        metadata.insert("width".to_string(), json!(width));
        metadata.insert("height".to_string(), json!(height));

        Ok(BinaryArtifact::with_metadata(
            response.image,
            "image/png",
            "png",
            format!("Model: {} | Size: {}x{}", self.model_name, width, height),
            metadata,
        ))
    }
}
