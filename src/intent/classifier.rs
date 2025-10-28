use std::fmt;

use crate::{
    config::{CapabilityRoute, OpenAiConfig, ZhipuConfig},
    errors::{AgentError, Result},
    providers::zhipu::ZhipuCompletionModel,
};
use rig::{
    agent::Agent,
    client::CompletionClient,
    completion::{Prompt, request::CompletionModel},
    providers::openai,
};
use serde::Deserialize;
use serde_json::Value;
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Conversation,
    ImageGeneration,
    MusicGeneration,
    VideoGeneration,
    Help,
    Unknown,
}

impl Intent {
    pub fn as_prefix(&self) -> &'static str {
        match self {
            Intent::Conversation => "chat",
            Intent::ImageGeneration => "image",
            Intent::MusicGeneration => "music",
            Intent::VideoGeneration => "video",
            Intent::Help => "help",
            Intent::Unknown => "unknown",
        }
    }

    fn from_str(value: &str) -> Self {
        match value.trim().to_lowercase().as_str() {
            "conversation" | "chat" | "dialogue" | "text" => Intent::Conversation,
            "image_generation" | "image" | "drawing" | "paint" | "art" => Intent::ImageGeneration,
            "music_generation" | "music" | "song" | "audio" => Intent::MusicGeneration,
            "video_generation" | "video" | "animation" | "film" => Intent::VideoGeneration,
            "help" | "support" => Intent::Help,
            _ => Intent::Unknown,
        }
    }
}

impl fmt::Display for Intent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_prefix())
    }
}

#[derive(Deserialize)]
struct IntentResponse {
    intent: String,
}

type OpenAiCompletionModel = <openai::Client as CompletionClient>::CompletionModel;

pub struct IntentClassifier {
    provider: Option<IntentProvider>,
}

enum IntentProvider {
    OpenAi {
        agent: Agent<OpenAiCompletionModel>,
    },
    Zhipu {
        model: ZhipuCompletionModel,
        system_prompt: String,
    },
}

const INTENT_ROUTER_SYSTEM_PROMPT: &str = "你是一名严格的路由器，只回答 JSON，格式为 {\"intent\": \"...\"}。intent 必须是 conversation、image_generation、music_generation、video_generation 或 help 之一。";

impl IntentClassifier {
    pub fn new(
        route: Option<&CapabilityRoute>,
        openai_client: Option<openai::Client>,
        openai_config: Option<&OpenAiConfig>,
        zhipu_config: Option<&ZhipuConfig>,
    ) -> Result<Self> {
        let provider = match route {
            None => None,
            Some(route) => match route.provider.as_str() {
                "openai" => {
                    let client = openai_client
                        .ok_or_else(|| AgentError::MissingConfig("openai.api_key (意图路由)"))?;
                    let cfg = openai_config
                        .ok_or_else(|| AgentError::MissingConfig("openai.chat_model (意图路由)"))?;
                    let model = route.model.as_deref().unwrap_or(&cfg.chat_model);
                    Some(IntentProvider::OpenAi {
                        agent: client
                            .agent(model)
                            .name("intent-router")
                            .preamble(INTENT_ROUTER_SYSTEM_PROMPT)
                            .build(),
                    })
                }
                "zhipu" => {
                    let cfg = zhipu_config
                        .ok_or_else(|| AgentError::MissingConfig("zhipu.api_key (意图路由)"))?;
                    let model = ZhipuCompletionModel::from_config(cfg, route.model.as_deref())
                        .map_err(|e| AgentError::other(format!("初始化智谱模型失败: {:?}", e)))?;
                    Some(IntentProvider::Zhipu {
                        model,
                        system_prompt: INTENT_ROUTER_SYSTEM_PROMPT.to_string(),
                    })
                }
                provider if provider.is_empty() || provider == "none" || provider == "disabled" => {
                    None
                }
                other => {
                    return Err(AgentError::unsupported(format!(
                        "未支持的意图路由提供方: {other}"
                    )));
                }
            },
        };

        Ok(Self { provider })
    }

    pub async fn classify(&self, input: &str) -> Result<Intent> {
        if input.trim().is_empty() {
            return Ok(Intent::Help);
        }

        if let Some(provider) = &self.provider {
            let prompt = format!(
                "请读取用户输入并判断其意图。只输出 JSON，形如 {{\"intent\": \"...\"}}，不输出额外内容。\n用户输入: ```{}```",
                input.trim()
            );

            match provider {
                IntentProvider::OpenAi { agent } => match agent.prompt(&prompt).await {
                    Ok(response) => {
                        if let Some(intent) = Self::parse_intent(&response) {
                            return Ok(intent);
                        }

                        warn!(
                            target: "intent_classifier",
                            response = %response,
                            "无法从模型返回中解析意图，改用关键字 fallback"
                        );
                    }
                    Err(err) => {
                        warn!(
                            target: "intent_classifier",
                            error = ?err,
                            "向模型请求意图失败，改用关键字 fallback"
                        );
                    }
                },
                IntentProvider::Zhipu {
                    model,
                    system_prompt,
                } => {
                    // 使用 rig 的 CompletionModel API
                    let request = model.completion_request(&prompt)
                        .preamble(system_prompt.clone())
                        .build();

                    match model.completion(request).await {
                        Ok(response) => {
                            // 从 response.choice 中提取文本
                            let text = response.choice.iter()
                                .filter_map(|content| match content {
                                    rig::completion::message::AssistantContent::Text(t) => Some(t.text.as_str()),
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                                .join("\n");

                            if let Some(intent) = Self::parse_intent(&text) {
                                return Ok(intent);
                            }

                            warn!(
                                target: "intent_classifier",
                                response = %text,
                                "无法从智谱返回中解析意图，改用关键字 fallback"
                            );
                        }
                        Err(err) => {
                            warn!(
                                target: "intent_classifier",
                                error = ?err,
                                "向智谱请求意图失败，改用关键字 fallback"
                            );
                        }
                    }
                }
            }
        }

        Ok(Self::fallback_intent(input))
    }

    fn parse_intent(response: &str) -> Option<Intent> {
        let trimmed = response.trim();
        let sanitized = if trimmed.starts_with("```json") {
            trimmed
                .trim_start_matches("```json")
                .trim_start_matches('`')
                .trim()
                .trim_end_matches("```")
                .trim()
        } else if trimmed.starts_with("```") {
            trimmed
                .trim_start_matches("```")
                .trim_end_matches("```")
                .trim()
        } else {
            trimmed
        };

        if let Ok(resp) = serde_json::from_str::<IntentResponse>(sanitized) {
            return Some(Intent::from_str(resp.intent.as_str()));
        }

        if let Ok(value) = serde_json::from_str::<Value>(sanitized) {
            if let Some(intent) = value.get("intent").and_then(Value::as_str) {
                return Some(Intent::from_str(intent));
            }
        }

        None
    }

    fn fallback_intent(input: &str) -> Intent {
        let normalized = input.to_lowercase();

        let chat_keywords = ["聊", "chat", "问", "explain", "说", "help"];
        if chat_keywords.iter().any(|k| normalized.contains(k)) {
            return Intent::Conversation;
        }

        let image_keywords = ["画", "image", "绘", "图", "picture", "logo", "design"];
        if image_keywords.iter().any(|k| normalized.contains(k)) {
            return Intent::ImageGeneration;
        }

        let music_keywords = ["music", "旋律", "歌曲", "歌", "伴奏", "和弦", "曲"];
        if music_keywords.iter().any(|k| normalized.contains(k)) {
            return Intent::MusicGeneration;
        }

        let video_keywords = ["视频", "video", "动画", "片段", "mv", "剪辑"];
        if video_keywords.iter().any(|k| normalized.contains(k)) {
            return Intent::VideoGeneration;
        }

        Intent::Conversation
    }
}
