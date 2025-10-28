use crate::{capabilities::BinaryArtifact, errors::Result};
use rig::providers::hyperbolic;

pub struct MusicGenerator {
    _client: hyperbolic::Client,
    _model_name: String,
    _voice: String,
}

impl MusicGenerator {
    pub fn new(client: hyperbolic::Client, model_name: &str, voice: &str) -> Self {
        Self {
            _client: client,
            _model_name: model_name.to_string(),
            _voice: voice.to_string(),
        }
    }

    pub async fn compose(&self, _prompt: &str) -> Result<BinaryArtifact> {
        // TODO: 修复 rig 0.22 的 AudioGeneration API
        // 当前版本的 API 结构与之前不同，需要查阅最新文档
        Err(crate::errors::AgentError::unsupported(
            "音乐生成功能暂时不可用，等待 rig-core 0.22 API 更新"
        ).into())
    }
}