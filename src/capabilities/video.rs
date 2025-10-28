use crate::{
    capabilities::BinaryArtifact,
    config::VideoConfig,
    errors::{AgentError, Result},
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Serialize};
use tracing::debug;

pub struct VideoGenerator {
    client: reqwest::Client,
    endpoint: String,
    api_key: Option<String>,
    format: String,
    max_duration_seconds: Option<u32>,
}

impl VideoGenerator {
    pub fn new(config: &VideoConfig, format_override: Option<&str>) -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::new(),
            endpoint: config.endpoint.clone(),
            api_key: config.api_key.clone(),
            format: format_override
                .map(|value| value.to_string())
                .unwrap_or_else(|| config.format.clone()),
            max_duration_seconds: config.max_duration_seconds,
        })
    }

    pub async fn generate(&self, prompt: &str) -> Result<BinaryArtifact> {
        let request_body = VideoGenerationRequest {
            prompt,
            format: &self.format,
            max_duration_seconds: self.max_duration_seconds,
        };

        let mut builder = self.client.post(&self.endpoint).json(&request_body);

        if let Some(api_key) = &self.api_key {
            builder = builder.bearer_auth(api_key);
        }

        let response = builder.send().await?;
        if !response.status().is_success() {
            return Err(AgentError::other(format!(
                "视频生成服务返回状态码 {}",
                response.status()
            )));
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();

        if content_type.contains("application/json") {
            let body = response.json::<VideoGenerationResponse>().await?;
            self.from_json(body).await
        } else {
            let bytes = response.bytes().await?.to_vec();
            Ok(BinaryArtifact::new(
                bytes,
                content_type,
                self.format.clone(),
                "External video service",
            ))
        }
    }

    async fn from_json(&self, payload: VideoGenerationResponse) -> Result<BinaryArtifact> {
        if let Some(b64) = &payload.video_base64 {
            let data = BASE64_STANDARD
                .decode(b64)
                .map_err(|err| AgentError::other(format!("视频 Base64 解码失败: {err}")))?;
            return Ok(BinaryArtifact::new(
                data,
                payload
                    .content_type
                    .as_deref()
                    .unwrap_or("video/mp4")
                    .to_string(),
                payload
                    .ext
                    .as_deref()
                    .unwrap_or_else(|| self.format.as_str())
                    .to_string(),
                payload
                    .summary
                    .clone()
                    .unwrap_or_else(|| "Video plan".to_string()),
            ));
        }

        if let Some(url) = &payload.video_url {
            debug!(target: "video_generator", "fetching video from {url}");
            let bytes = self.client.get(url).send().await?.bytes().await?.to_vec();
            return Ok(BinaryArtifact::new(
                bytes,
                payload
                    .content_type
                    .as_deref()
                    .unwrap_or("video/mp4")
                    .to_string(),
                payload
                    .ext
                    .as_deref()
                    .unwrap_or_else(|| self.format.as_str())
                    .to_string(),
                payload
                    .summary
                    .clone()
                    .unwrap_or_else(|| "Video plan".to_string()),
            ));
        }

        Err(AgentError::unsupported(
            "视频服务返回结果缺少 video_base64 或 video_url 字段",
        ))
    }
}

#[derive(Serialize)]
struct VideoGenerationRequest<'a> {
    prompt: &'a str,
    format: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_duration_seconds: Option<u32>,
}

#[derive(Deserialize, Debug)]
struct VideoGenerationResponse {
    #[serde(default)]
    video_base64: Option<String>,
    #[serde(default)]
    video_url: Option<String>,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    ext: Option<String>,
    #[serde(default)]
    summary: Option<String>,
}
