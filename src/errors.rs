use thiserror::Error;

pub type Result<T> = std::result::Result<T, AgentError>;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("缺少必要的配置: {0}")]
    MissingConfig(&'static str),

    #[error("当前能力暂不可用: {0}")]
    Unsupported(String),

    #[error("I/O 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("网络请求失败: {0}")]
    Reqwest(#[from] reqwest::Error),

    #[error("JSON 解析失败: {0}")]
    SerdeJson(#[from] serde_json::Error),

    #[error("LLM 请求失败: {0}")]
    Prompt(#[from] rig::completion::PromptError),

    #[error("文本生成失败: {0}")]
    Completion(#[from] rig::completion::CompletionError),

    #[error("音频生成失败: {0}")]
    AudioGeneration(#[from] rig::audio_generation::AudioGenerationError),

    #[error("图像生成失败: {0}")]
    ImageGeneration(#[from] rig::image_generation::ImageGenerationError),

    #[error("内部错误: {0}")]
    Other(String),
}

impl AgentError {
    pub fn unsupported(feature: impl Into<String>) -> Self {
        Self::Unsupported(feature.into())
    }

    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}

impl From<anyhow::Error> for AgentError {
    fn from(value: anyhow::Error) -> Self {
        Self::Other(value.to_string())
    }
}
