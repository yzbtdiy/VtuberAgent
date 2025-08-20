use anyhow::Result;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid;

use crate::agents::DanmakuAgents;
use crate::config::Settings;
use crate::memory::{LongTermMemory, MemoryItem};
use crate::models::{DanmakuProcessingResult, IntentType, WebSocketMessage};
use crate::tools::{ImageGenerationTool, TTSTool};

pub type ProgressSender = mpsc::UnboundedSender<WebSocketMessage>;

#[derive(Clone)]
pub struct DanmakuWorkflow {
    agents: Arc<DanmakuAgents>,
    image_tool: Arc<ImageGenerationTool>,
    tts_tool: Arc<TTSTool>,
    memory: Arc<LongTermMemory>,
    settings: Arc<Settings>,
}

impl DanmakuWorkflow {
    pub async fn new(settings: &Settings) -> Result<Self> {
        let agents = Arc::new(DanmakuAgents::new(settings));
        let image_tool = Arc::new(ImageGenerationTool::new(settings));
        let tts_tool = Arc::new(TTSTool::new(settings));
        let memory = Arc::new(LongTermMemory::new(settings.long_term_memory.clone()).await?);
        let settings = Arc::new(settings.clone());

        Ok(Self {
            agents,
            image_tool,
            tts_tool,
            memory,
            settings,
        })
    }

    pub async fn process_danmaku(
        &self,
        danmaku_content: &str,
        user_id: &str,
        progress_sender: Option<ProgressSender>,
    ) -> Result<DanmakuProcessingResult> {
        info!("Processing danmaku: {}", danmaku_content);

        // Validate input length
        if danmaku_content.len() > self.settings.processing.max_danmaku_length {
            return Err(anyhow::anyhow!("Danmaku content too long"));
        }

        // Step 0: Retrieve relevant context from long-term memory (if enabled)
        let mut context = String::new();
        if self.memory.is_enabled() {
            self.send_progress(
                &progress_sender,
                "memory_retrieval",
                "🧠 正在检索相关记忆...",
                None,
            )
            .await;

            if let Ok(embeddings) = self.generate_embeddings(danmaku_content).await {
                if let Ok(memory_context) = self
                    .memory
                    .retrieve_relevant_context(embeddings, user_id)
                    .await
                {
                    if !memory_context.is_empty() {
                        context = format!("相关记忆:\n{}\n\n", memory_context.join("\n"));
                        info!(
                            "Retrieved {} relevant memories for context",
                            memory_context.len()
                        );
                    }
                }
            }
        }

        // Step 1: Intent Analysis
        self.send_progress(
            &progress_sender,
            "intent_analysis",
            "🤔 正在分析弹幕意图...",
            None,
        )
        .await;

        let intent_type = self.agents.analyze_intent(danmaku_content).await?;
        info!("Detected intent type: {:?}", intent_type);

        // Step 2: Generate Response (with context)
        self.send_progress(
            &progress_sender,
            "response_generation",
            "💭 正在生成回应内容...",
            None,
        )
        .await;

        let enhanced_input = if context.is_empty() {
            danmaku_content.to_string()
        } else {
            format!("{}{}", context, danmaku_content)
        };

        let (text_response, image_prompt) = match intent_type {
            IntentType::Conversation => {
                let response = self
                    .agents
                    .generate_conversation_response(&enhanced_input)
                    .await?;
                (response, None)
            }
            IntentType::SingingRequest => {
                let response = self
                    .agents
                    .generate_singing_response(&enhanced_input)
                    .await?;
                (response, None)
            }
            IntentType::DrawingRequest => {
                let (response, prompt) = self
                    .agents
                    .generate_drawing_response(&enhanced_input)
                    .await?;
                (response, Some(prompt))
            }
            IntentType::OtherCommand => {
                let response = self.agents.generate_other_response(&enhanced_input).await?;
                (response, None)
            }
        };

        // Step 3: Generate Image (if needed)
        let image_url = if let Some(image_prompt) = image_prompt {
            self.send_progress(
                &progress_sender,
                "image_generation_start",
                "🎨 正在为您创作图片，请稍等片刻...",
                Some(image_prompt.clone()),
            )
            .await;

            match self.generate_image(&image_prompt, &progress_sender).await {
                Ok(data) => {
                    self.send_progress(
                        &progress_sender,
                        "image_generation_complete",
                        "✨ 图片创作完成！",
                        None,
                    )
                    .await;
                    Some(data)
                }
                Err(e) => {
                    error!("Image generation failed: {}", e);
                    self.send_progress(
                        &progress_sender,
                        "image_generation_error",
                        "❌ 图片生成失败，请稍后再试",
                        None,
                    )
                    .await;
                    None
                }
            }
        } else {
            None
        };

        // Step 4: Generate Audio
        self.send_progress(
            &progress_sender,
            "tts_generation",
            "🎤 正在生成语音回应...",
            None,
        )
        .await;

        let audio_data = match self.tts_tool.generate_speech(&text_response).await {
            Ok(data) => {
                self.send_progress(&progress_sender, "tts_complete", "🔊 语音生成完成！", None)
                    .await;
                Some(data)
            }
            Err(e) => {
                error!("TTS generation failed: {}", e);
                self.send_progress(&progress_sender, "tts_error", "❌ 语音生成失败", None)
                    .await;
                None
            }
        };

        // Step 5: Store interaction in long-term memory (if enabled)
        if self.memory.is_enabled() {
            self.send_progress(
                &progress_sender,
                "memory_storage",
                "💾 正在保存交互记忆...",
                None,
            )
            .await;

            if let Ok(embeddings) = self.generate_embeddings(danmaku_content).await {
                let memory_item = MemoryItem {
                    id: uuid::Uuid::new_v4().to_string(),
                    user_id: user_id.to_string(),
                    content: danmaku_content.to_string(),
                    intent: format!("{:?}", intent_type),
                    timestamp: chrono::Utc::now(),
                    context: if context.is_empty() {
                        None
                    } else {
                        Some(context.clone())
                    },
                };

                if let Err(e) = self.memory.store_interaction(memory_item, embeddings).await {
                    warn!("Failed to store memory: {}", e);
                } else {
                    info!("Successfully stored interaction in long-term memory");
                }
            }
        }

        // Step 6: Complete
        self.send_progress(
            &progress_sender,
            "processing_complete",
            "✅ 处理完成！",
            None,
        )
        .await;

        Ok(DanmakuProcessingResult {
            intent_type,
            text_response,
            audio_data,
            image_url,
        })
    }

    async fn generate_image(
        &self,
        prompt: &str,
        progress_sender: &Option<ProgressSender>,
    ) -> Result<String> {
        self.send_progress(
            progress_sender,
            "image_prompt_optimization",
            "✨ 正在优化绘画提示词...",
            None,
        )
        .await;

        let optimized_prompt = self.image_tool.optimize_prompt(prompt);

        self.send_progress(
            progress_sender,
            "image_generation_progress",
            "🎨 AI正在努力创作中，精美的画作马上就好...",
            Some(optimized_prompt.clone()),
        )
        .await;

        let image_url = self.image_tool.generate_image(&optimized_prompt).await?;

        Ok(image_url)
    }

    async fn send_progress(
        &self,
        progress_sender: &Option<ProgressSender>,
        stage: &str,
        message: &str,
        image_prompt: Option<String>,
    ) {
        if let Some(sender) = progress_sender {
            let progress_msg = WebSocketMessage::Progress {
                stage: stage.to_string(),
                message: message.to_string(),
                image_prompt,
            };

            if let Err(e) = sender.send(progress_msg) {
                warn!("Failed to send progress update: {}", e);
            }
        }
    }

    async fn generate_embeddings(&self, text: &str) -> Result<Vec<f32>> {
        // Use the same agent to generate embeddings for the text
        // This is a simplified approach - in production, you might want to use a dedicated embedding model

        // For now, we'll create a simple hash-based embedding as a placeholder
        // In a real implementation, you would use OpenAI's embedding API or another embedding service
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let hash = hasher.finish();

        // Convert hash to a vector of floats (1536 dimensions for OpenAI text-embedding-3-small)
        let mut embeddings = Vec::with_capacity(1536);
        let mut seed = hash;

        for _ in 0..1536 {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let value = ((seed / 65536) % 32768) as f32 / 32768.0 - 1.0;
            embeddings.push(value);
        }

        Ok(embeddings)
    }
}
