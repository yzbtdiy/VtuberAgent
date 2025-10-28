use std::time::Duration;

use crate::config::ZhipuConfig;
use futures_util::StreamExt as FuturesStreamExt;
use reqwest::Client as HttpClient;
use rig::completion::{
    message::AssistantContent,
    request::{CompletionRequest, CompletionError, CompletionResponse, CompletionModel, GetTokenUsage, Usage},
    Message,
};
use rig::one_or_many::OneOrMany;
use rig::streaming::RawStreamingChoice;
use serde::{Deserialize, Serialize};

/// 智谱 AI GLM 系列模型的 Completion 实现
/// 
/// 支持功能：
/// - 标准 completion() 方法：一次性返回完整响应
/// - 流式 stream() 方法：增量返回 token，适用于实时交互场景
/// 
/// # 使用示例
/// 
/// ## 标准模式
/// ```no_run
/// use rig::completion::CompletionModel;
/// 
/// let model = ZhipuCompletionModel::from_config(&config, None)?;
/// let request = model.completion_request("你好").build();
/// let response = model.completion(request).await?;
/// println!("{}", response.choice.first().unwrap().text());
/// ```
/// 
/// ## 流式模式
/// ```no_run
/// use futures_util::StreamExt;
/// use rig::completion::CompletionModel;
/// 
/// let model = ZhipuCompletionModel::from_config(&config, None)?;
/// let request = model.completion_request("讲一个故事").build();
/// let mut stream = model.stream(request).await?;
/// 
/// // 逐个 token 打印
/// while let Some(result) = stream.next().await {
///     match result? {
///         rig::streaming::StreamedAssistantContent::Text(text) => {
///             print!("{}", text);
///         }
///         _ => {}
///     }
/// }
/// ```
#[derive(Clone)]
pub struct ZhipuCompletionModel {
    http_client: HttpClient,
    api_key: String,
    model: String,
    endpoint: String,
}

impl ZhipuCompletionModel {
    pub fn from_config(config: &ZhipuConfig, model_override: Option<&str>) -> Result<Self, CompletionError> {
        let http_client = HttpClient::builder()
            .user_agent("VutberAgent/0.1")
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| CompletionError::ProviderError(e.to_string()))?;

        Ok(Self {
            http_client,
            api_key: config.api_key.clone(),
            model: model_override
                .map(|value| value.to_string())
                .unwrap_or_else(|| config.chat_model.clone()),
            endpoint: config.api_url.clone(),
        })
    }
}

impl CompletionModel for ZhipuCompletionModel {
    type Response = ZhipuChatResponse;
    type StreamingResponse = ZhipuChatResponse; // 支持流式响应

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> std::result::Result<CompletionResponse<Self::Response>, CompletionError> {
        // 转换 rig 的 Message 为 Zhipu 格式
        let messages: Vec<ZhipuRequestMessage> = request
            .chat_history
            .iter()
            .filter_map(|msg| match msg {
                Message::User { content } => {
                    // 提取文本内容
                    let text = content
                        .iter()
                        .filter_map(|c| match c {
                            rig::completion::message::UserContent::Text(t) => Some(t.text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    if text.is_empty() {
                        None
                    } else {
                        Some(ZhipuRequestMessage {
                            role: "user",
                            content: text,
                        })
                    }
                }
                Message::Assistant { content, .. } => {
                    // 提取助手文本
                    let text = content
                        .iter()
                        .filter_map(|c| match c {
                            rig::completion::message::AssistantContent::Text(t) => Some(t.text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    if text.is_empty() {
                        None
                    } else {
                        Some(ZhipuRequestMessage {
                            role: "assistant",
                            content: text,
                        })
                    }
                }
                // Message 枚举目前只有 User 和 Assistant 两个变体
            })
            .collect();

        if messages.is_empty() {
            return Err(CompletionError::RequestError(
                "No valid messages in request".into(),
            ));
        }

        let payload = ZhipuChatRequest {
            model: &self.model,
            messages: &messages,
        };

        let response = self
            .http_client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&payload)
            .send()
            .await
            .map_err(|e| CompletionError::ProviderError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CompletionError::ProviderError(format!(
                "智谱 API 请求失败 ({}): {}",
                status, body
            )));
        }

        let zhipu_response: ZhipuChatResponse = response
            .json()
            .await
            .map_err(|e| CompletionError::ProviderError(format!("解析响应失败: {}", e)))?;

        // 提取文本并构造 rig 的响应
        let text = zhipu_response
            .extract_text()
            .ok_or_else(|| CompletionError::ProviderError("智谱 API 返回结果为空".to_string()))?;

        Ok(CompletionResponse {
            choice: OneOrMany::one(AssistantContent::Text(text.into())),
            raw_response: zhipu_response.clone(),
            usage: zhipu_response.token_usage().unwrap_or_default(),
        })
    }

    async fn stream(
        &self,
        request: CompletionRequest,
    ) -> std::result::Result<rig::streaming::StreamingCompletionResponse<Self::StreamingResponse>, CompletionError> {
        // 转换 rig 的 Message 为 Zhipu 格式
        let messages: Vec<ZhipuRequestMessage> = request
            .chat_history
            .iter()
            .filter_map(|msg| match msg {
                Message::User { content } => {
                    let text = content
                        .iter()
                        .filter_map(|c| match c {
                            rig::completion::message::UserContent::Text(t) => Some(t.text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    if text.is_empty() {
                        None
                    } else {
                        Some(ZhipuRequestMessage {
                            role: "user",
                            content: text,
                        })
                    }
                }
                Message::Assistant { content, .. } => {
                    let text = content
                        .iter()
                        .filter_map(|c| match c {
                            rig::completion::message::AssistantContent::Text(t) => Some(t.text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    if text.is_empty() {
                        None
                    } else {
                        Some(ZhipuRequestMessage {
                            role: "assistant",
                            content: text,
                        })
                    }
                }
            })
            .collect();

        if messages.is_empty() {
            return Err(CompletionError::RequestError(
                "No valid messages in request".into(),
            ));
        }

        // 添加 stream: true 参数
        let payload = ZhipuStreamRequest {
            model: &self.model,
            messages: &messages,
            stream: true,
        };

        let response = self
            .http_client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&payload)
            .send()
            .await
            .map_err(|e| CompletionError::ProviderError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CompletionError::ProviderError(format!(
                "智谱 API 流式请求失败 ({}): {}",
                status, body
            )));
        }

        // 创建 SSE 流
        let stream = response.bytes_stream().map(move |chunk_result| {
            let chunk = chunk_result.map_err(|e| CompletionError::ProviderError(e.to_string()))?;
            let text = String::from_utf8_lossy(&chunk);
            
            // 解析 SSE 格式: data: {...}
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with("data:") {
                    let json_str = line.strip_prefix("data:").unwrap().trim();
                    if json_str == "[DONE]" {
                        continue;
                    }
                    
                    // 解析智谱 AI 的流式响应
                    match serde_json::from_str::<ZhipuStreamChunk>(json_str) {
                        Ok(chunk) => {
                            // 提取文本增量
                            if let Some(choice) = chunk.choices.first() {
                                if let Some(ref delta) = choice.delta {
                                    if let Some(ref content) = delta.content {
                                        return Ok(RawStreamingChoice::<ZhipuChatResponse>::Message(content.clone()));
                                    }
                                }
                            }
                            // 如果是最后一个块，返回完整响应(带 usage 信息)
                            if chunk.choices.iter().any(|c| c.finish_reason.is_some()) {
                                // 构造完整响应
                                let full_response = ZhipuChatResponse {
                                    choices: vec![],  // 流式响应不需要完整的 choices
                                    usage: chunk.usage,
                                };
                                return Ok(RawStreamingChoice::FinalResponse(full_response));
                            }
                        }
                        Err(e) => {
                            return Err(CompletionError::ProviderError(format!(
                                "解析智谱流式响应失败: {}",
                                e
                            )));
                        }
                    }
                }
            }
            
            // 如果没有解析到任何内容，跳过此块
            Err(CompletionError::ProviderError("无效的 SSE 数据块".to_string()))
        })
        .filter_map(|result| async move {
            match result {
                Ok(choice) => Some(Ok(choice)),
                Err(e) if e.to_string().contains("无效的 SSE 数据块") => None,
                Err(e) => Some(Err(e)),
            }
        });

        Ok(rig::streaming::StreamingCompletionResponse::stream(Box::pin(stream)))
    }
}

#[derive(Serialize)]
struct ZhipuChatRequest<'a> {
    model: &'a str,
    messages: &'a [ZhipuRequestMessage<'a>],
}

#[derive(Serialize)]
struct ZhipuStreamRequest<'a> {
    model: &'a str,
    messages: &'a [ZhipuRequestMessage<'a>],
    stream: bool,
}

#[derive(Deserialize)]
struct ZhipuStreamChunk {
    choices: Vec<ZhipuStreamChoice>,
    usage: Option<ZhipuUsage>,
}

#[derive(Deserialize)]
struct ZhipuStreamChoice {
    delta: Option<ZhipuDelta>,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ZhipuDelta {
    content: Option<String>,
}

#[derive(Serialize)]
struct ZhipuRequestMessage<'a> {
    role: &'a str,
    content: String,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct ZhipuChatResponse {
    #[serde(default)]
    choices: Vec<ZhipuChoice>,
    #[serde(default)]
    usage: Option<ZhipuUsage>,
}

impl ZhipuChatResponse {
    fn extract_text(&self) -> Option<String> {
        self.choices
            .iter()
            .find_map(|choice| choice.message.as_ref()?.extract_text())
    }
}

impl GetTokenUsage for ZhipuChatResponse {
    fn token_usage(&self) -> Option<Usage> {
        self.usage.as_ref().map(|u| Usage {
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
        })
    }
}

// 智谱AI的token使用统计
#[derive(Deserialize, Serialize, Clone)]
struct ZhipuUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
}

#[derive(Deserialize, Serialize, Clone)]
struct ZhipuChoice {
    #[serde(default)]
    message: Option<ZhipuMessage>,
}

#[derive(Deserialize, Serialize, Clone)]
struct ZhipuMessage {
    #[serde(default)]
    content: Option<ZhipuMessageContent>,
}

impl ZhipuMessage {
    fn extract_text(&self) -> Option<String> {
        self.content.as_ref()?.extract_text()
    }
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(untagged)]
enum ZhipuMessageContent {
    Text(String),
    Segments(Vec<ZhipuMessageSegment>),
}

impl ZhipuMessageContent {
    fn extract_text(&self) -> Option<String> {
        match self {
            ZhipuMessageContent::Text(text) => Some(text.clone()),
            ZhipuMessageContent::Segments(segments) => {
                if segments.is_empty() {
                    None
                } else {
                    Some(
                        segments
                            .iter()
                            .filter_map(|segment| segment.text.as_ref())
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(""),
                    )
                }
            }
        }
    }
}

#[derive(Deserialize, Serialize, Clone)]
struct ZhipuMessageSegment {
    #[serde(rename = "type")]
    _kind: Option<String>,
    #[serde(default)]
    text: Option<String>,
}
