use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WebSocketMessage {
    #[serde(rename = "connected")]
    Connected {
        message: String,
        auth_required: bool,
        auth_status: String,
    },
    #[serde(rename = "auth")]
    Auth { auth_data: AuthData },
    #[serde(rename = "auth_success")]
    AuthSuccess {
        message: String,
        user_id: String,
        auth_type: String,
    },
    #[serde(rename = "auth_required")]
    AuthRequired { message: String },
    #[serde(rename = "danmaku")]
    Danmaku {
        content: String,
        user_id: String,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "danmaku_result")]
    DanmakuResult {
        success: bool,
        original_danmaku: String,
        intent_type: String,
        text_response: String,
        has_audio: bool,
        has_image: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        audio_data: Option<String>, // base64 encoded
        #[serde(skip_serializing_if = "Option::is_none")]
        image_data: Option<String>, // base64 encoded
    },
    #[serde(rename = "progress")]
    Progress {
        stage: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        image_prompt: Option<String>,
    },
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "pong")]
    Pong,
    #[serde(rename = "error")]
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthData {
    #[serde(rename = "type")]
    pub auth_type: String, // "signature" or "api_key"
    pub user_id: String,
    pub api_key: String,
    pub timestamp: DateTime<Utc>,
    pub nonce: String,
    pub signature: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_data: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AuthenticatedClient {
    #[allow(dead_code)]
    pub id: Uuid,
    pub user_id: String,
    pub auth_type: String,
    #[allow(dead_code)]
    pub authenticated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IntentType {
    #[serde(rename = "对话聊天")]
    Conversation,
    #[serde(rename = "唱歌请求")]
    SingingRequest,
    #[serde(rename = "绘画请求")]
    DrawingRequest,
    #[serde(rename = "其他指令")]
    OtherCommand,
}

impl IntentType {
    pub fn as_str(&self) -> &'static str {
        match self {
            IntentType::Conversation => "对话聊天",
            IntentType::SingingRequest => "唱歌请求",
            IntentType::DrawingRequest => "绘画请求",
            IntentType::OtherCommand => "其他指令",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "对话聊天" => IntentType::Conversation,
            "唱歌请求" => IntentType::SingingRequest,
            "绘画请求" => IntentType::DrawingRequest,
            _ => IntentType::OtherCommand,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DanmakuProcessingResult {
    pub intent_type: IntentType,
    pub text_response: String,
    pub audio_data: Option<Vec<u8>>,
    pub image_url: Option<String>,
}
