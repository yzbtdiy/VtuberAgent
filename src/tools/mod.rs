use anyhow::{Result, anyhow};
use reqwest::Client;
use serde_json::{Value, json};

use crate::config::Settings;

#[derive(Clone)]
pub struct ImageGenerationTool {
    client: Client,
    base_url: String,
    api_key: String,
}

impl ImageGenerationTool {
    pub fn new(settings: &Settings) -> Self {
        Self {
            client: Client::new(),
            base_url: settings.openai.base_url.clone(),
            api_key: settings.openai.api_key.clone(),
        }
    }

    pub async fn generate_image(&self, prompt: &str) -> Result<String> {
        let url = format!("{}/images/generations", self.base_url);

        let payload = json!({
            "model": "flux.1.1-pro",
            "prompt": prompt,
            "n": 1,
            "size": "1024x1024",
            "response_format": "url"
        });

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow!("Image generation failed: {}", error_text));
        }

        let response_json: Value = response.json().await?;

        // Debug: print the response structure
        eprintln!(
            "Image API Response: {}",
            serde_json::to_string_pretty(&response_json)?
        );

        // Extract URL from the response
        if let Some(data_array) = response_json.get("data").and_then(|d| d.as_array()) {
            if let Some(first_item) = data_array.first() {
                if let Some(image_url) = first_item.get("url").and_then(|s| s.as_str()) {
                    return Ok(image_url.to_string());
                }
            }
        }

        Err(anyhow!(
            "No image URL found in response. Available fields: {:?}",
            response_json
                .as_object()
                .map(|o| o.keys().collect::<Vec<_>>())
                .unwrap_or_default()
        ))
    }

    pub fn optimize_prompt(&self, prompt: &str) -> String {
        // Basic prompt optimization
        let optimized = format!(
            "High quality, detailed illustration of {}. Digital art, vibrant colors, professional artwork.",
            prompt.trim()
        );
        optimized
    }

    #[allow(dead_code)]
    pub fn extract_image_prompt(&self, text: &str) -> Option<String> {
        // Simple extraction logic - look for drawing-related keywords
        let drawing_keywords = [
            "画", "绘", "画一", "画个", "画只", "画张", "画幅", "draw", "paint", "create", "make",
            "generate",
        ];

        for keyword in &drawing_keywords {
            if text.contains(keyword) {
                // Extract the content after the keyword
                if let Some(pos) = text.find(keyword) {
                    let after_keyword = &text[pos + keyword.len()..];
                    return Some(after_keyword.trim().to_string());
                }
            }
        }

        // Fallback: return the original text
        Some(text.to_string())
    }
}

#[derive(Clone)]
pub struct TTSTool {
    client: Client,
    openai_config: OpenAITTSConfig,
    indextts_config: Option<IndexTTSConfig>,
    use_indextts: bool,
}

#[derive(Clone)]
struct OpenAITTSConfig {
    base_url: String,
    api_key: String,
    model: String,
    voice: String,
}

#[derive(Clone)]
struct IndexTTSConfig {
    url: String,
    model: String,
    voice: String,
}

impl TTSTool {
    pub fn new(settings: &Settings) -> Self {
        let openai_config = OpenAITTSConfig {
            base_url: settings.openai.base_url.clone(),
            api_key: settings.openai.api_key.clone(),
            model: settings.openai.tts_model.clone(),
            voice: settings.openai.tts_voice.clone(),
        };

        let indextts_config = if settings.openai.use_indextts {
            Some(IndexTTSConfig {
                url: settings.indextts.url.clone(),
                model: settings.indextts.model.clone(),
                voice: settings.indextts.voice.clone(),
            })
        } else {
            None
        };

        Self {
            client: Client::new(),
            openai_config,
            indextts_config,
            use_indextts: settings.openai.use_indextts,
        }
    }

    pub async fn generate_speech(&self, text: &str) -> Result<Vec<u8>> {
        if self.use_indextts {
            self.generate_speech_indextts(text).await
        } else {
            self.generate_speech_openai(text).await
        }
    }

    async fn generate_speech_openai(&self, text: &str) -> Result<Vec<u8>> {
        let url = format!("{}/audio/speech", self.openai_config.base_url);

        let payload = json!({
            "model": self.openai_config.model,
            "input": text,
            "voice": self.openai_config.voice,
            "response_format": "mp3"
        });

        let response = self
            .client
            .post(&url)
            .header(
                "Authorization",
                format!("Bearer {}", self.openai_config.api_key),
            )
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow!("OpenAI TTS generation failed: {}", error_text));
        }

        let audio_data = response.bytes().await?;
        Ok(audio_data.to_vec())
    }

    async fn generate_speech_indextts(&self, text: &str) -> Result<Vec<u8>> {
        let indextts_config = self
            .indextts_config
            .as_ref()
            .ok_or_else(|| anyhow!("IndexTTS config not available"))?;

        let url = format!("{}/audio/speech", indextts_config.url);

        let payload = json!({
            "model": indextts_config.model,
            "input": text,
            "voice": indextts_config.voice
        });

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow!("IndexTTS generation failed: {}", error_text));
        }

        let audio_data = response.bytes().await?;
        Ok(audio_data.to_vec())
    }
}
