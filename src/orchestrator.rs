use std::path::PathBuf;

use crate::{
    capabilities::{ConversationAgent, ImageGenerator, MusicGenerator, VideoGenerator},
    config::AppConfig,
    errors::{AgentError, Result},
    intent::{Intent, IntentClassifier},
    live::{LiveEvent, LiveManager, LiveSessionInfo},
    util::ArtifactWriter,
};
use rig::{
    client::CompletionClient,
    providers::{hyperbolic, openai},
};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use tracing::{info, warn};

pub enum ExecutionOutcome {
    Conversation {
        response: String,
    },
    Artifact {
        intent: Intent,
        path: PathBuf,
        description: String,
    },
    Help {
        message: String,
    },
}

impl ExecutionOutcome {
    pub fn as_event_payload(&self) -> (&'static str, serde_json::Value) {
        match self {
            ExecutionOutcome::Conversation { response } => (
                "agent.conversation",
                json!({
                    "response": response,
                }),
            ),
            ExecutionOutcome::Artifact {
                intent,
                path,
                description,
            } => (
                "agent.artifact",
                json!({
                    "intent": intent.to_string(),
                    "path": path.to_string_lossy(),
                    "description": description,
                }),
            ),
            ExecutionOutcome::Help { message } => (
                "agent.help",
                json!({
                    "message": message,
                }),
            ),
        }
    }
}

pub struct AgentController {
    classifier: IntentClassifier,
    conversation: Option<ConversationAgent>,
    image: Option<ImageGenerator>,
    music: Option<MusicGenerator>,
    video: Option<VideoGenerator>,
    writer: ArtifactWriter,
    live: Option<LiveManager>,
    live_event_rx: Option<mpsc::Receiver<LiveEvent>>,
    broadcaster: Option<broadcast::Sender<String>>,
}

impl AgentController {
    pub async fn new(
        config: AppConfig,
        broadcaster: Option<broadcast::Sender<String>>,
    ) -> Result<Self> {
        let AppConfig {
            openai,
            hyperbolic,
            video,
            zhipu,
            bilibili_live,
            providers,
            artifacts_dir,
            sse: _,
        } = config;

        let writer = ArtifactWriter::new(artifacts_dir).await?;

        let openai_client = if let Some(cfg) = openai.as_ref() {
            let mut builder = openai::Client::builder(&cfg.api_key);
            if let Some(base_url) = cfg.base_url.as_deref() {
                builder = builder.base_url(base_url);
            }
            Some(builder.build())
        } else {
            None
        };

        let hyperbolic_client = hyperbolic
            .as_ref()
            .map(|cfg| hyperbolic::Client::new(&cfg.api_key));

        let classifier = IntentClassifier::new(
            providers.intent.as_ref(),
            openai_client.clone(),
            openai.as_ref(),
            zhipu.as_ref(),
        )?;

        let conversation = match providers.conversation.as_ref() {
            Some(route) => match route.provider.as_str() {
                "openai" => {
                    let client = openai_client
                        .clone()
                        .ok_or_else(|| AgentError::MissingConfig("openai.api_key (聊天)"))?;
                    let cfg = openai
                        .as_ref()
                        .ok_or_else(|| AgentError::MissingConfig("openai.chat_model (聊天)"))?;
                    let model = route.model.as_deref().unwrap_or(&cfg.chat_model);
                    let agent = client
                        .agent(model)
                        .name("vutber-conversation")
                        .preamble(&cfg.agent_preamble)
                        .build();
                    Some(ConversationAgent::with_openai(agent))
                }
                "zhipu" => {
                    let cfg = zhipu
                        .as_ref()
                        .ok_or_else(|| AgentError::MissingConfig("zhipu.api_key (聊天)"))?;
                    Some(ConversationAgent::with_zhipu(cfg, route.model.as_deref())?)
                }
                provider if provider.is_empty() || provider == "none" || provider == "disabled" => {
                    None
                }
                other => {
                    return Err(AgentError::unsupported(format!(
                        "未支持的对话提供方: {other}"
                    )));
                }
            },
            None => None,
        };

        let image = match providers.image.as_ref() {
            Some(route) => match route.provider.as_str() {
                "openai" => {
                    let client = openai_client
                        .clone()
                        .ok_or_else(|| AgentError::MissingConfig("openai.api_key (绘画生成)"))?;
                    let cfg = openai.as_ref().ok_or_else(|| {
                        AgentError::MissingConfig("openai.image_model (绘画生成)")
                    })?;
                    let model = route.model.as_deref().unwrap_or(&cfg.image_model);
                    Some(ImageGenerator::new(client, model))
                }
                provider if provider.is_empty() || provider == "none" || provider == "disabled" => {
                    None
                }
                other => {
                    return Err(AgentError::unsupported(format!(
                        "未支持的图像生成提供方: {other}"
                    )));
                }
            },
            None => None,
        };

        let music = match providers.music.as_ref() {
            Some(route) => match route.provider.as_str() {
                "hyperbolic" => {
                    let client = hyperbolic_client.clone().ok_or_else(|| {
                        AgentError::MissingConfig("hyperbolic.api_key (音乐生成)")
                    })?;
                    let cfg = hyperbolic.as_ref().ok_or_else(|| {
                        AgentError::MissingConfig("hyperbolic.language (音乐生成)")
                    })?;
                    let model = route.model.as_deref().unwrap_or(&cfg.language);
                    Some(MusicGenerator::new(client, model, &cfg.voice))
                }
                provider if provider.is_empty() || provider == "none" || provider == "disabled" => {
                    None
                }
                other => {
                    return Err(AgentError::unsupported(format!(
                        "未支持的音乐生成提供方: {other}"
                    )));
                }
            },
            None => None,
        };

        let video = match providers.video.as_ref() {
            Some(route) => match route.provider.as_str() {
                "custom" => {
                    let cfg = video
                        .as_ref()
                        .ok_or_else(|| AgentError::MissingConfig("video.endpoint (视频生成)"))?;
                    Some(VideoGenerator::new(cfg, route.model.as_deref())?)
                }
                provider if provider.is_empty() || provider == "none" || provider == "disabled" => {
                    None
                }
                other => {
                    return Err(AgentError::unsupported(format!(
                        "未支持的视频生成提供方: {other}"
                    )));
                }
            },
            None => None,
        };

        let (live, live_event_rx) = match bilibili_live {
            Some(cfg) => {
                let (tx, rx) = mpsc::channel(64);
                (
                    Some(LiveManager::new(cfg, Some(tx), broadcaster.clone())),
                    Some(rx),
                )
            }
            None => (None, None),
        };

        Ok(Self {
            classifier,
            conversation,
            image,
            music,
            video,
            writer,
            live,
            live_event_rx,
            broadcaster,
        })
    }

    pub fn capabilities_overview(&self) -> Vec<(Intent, bool)> {
        vec![
            (Intent::Conversation, self.conversation.is_some()),
            (Intent::ImageGeneration, self.image.is_some()),
            (Intent::MusicGeneration, self.music.is_some()),
            (Intent::VideoGeneration, self.video.is_some()),
        ]
    }

    pub fn has_live_listener(&self) -> bool {
        self.live_event_rx.is_some()
    }

    pub async fn recv_live_event(&mut self) -> Option<LiveEvent> {
        if self.live_event_rx.is_none() {
            return None;
        }

        let result = {
            let receiver = self.live_event_rx.as_mut().expect("checked is_some");
            receiver.recv().await
        };

        if result.is_none() {
            self.live_event_rx = None;
        }

        result
    }

    pub async fn start_live(&mut self) -> Result<LiveSessionInfo> {
        let manager = self
            .live
            .as_mut()
            .ok_or_else(|| AgentError::MissingConfig("live.bilibili"))?;
        manager.start().await
    }

    pub async fn stop_live(&mut self) -> Result<Option<LiveSessionInfo>> {
        let manager = self
            .live
            .as_mut()
            .ok_or_else(|| AgentError::MissingConfig("live.bilibili"))?;
        manager.stop().await
    }

    pub fn live_status(&self) -> Result<Option<LiveSessionInfo>> {
        let manager = self
            .live
            .as_ref()
            .ok_or_else(|| AgentError::MissingConfig("live.bilibili"))?;
        Ok(manager.info())
    }

    pub async fn handle_live_event(&mut self, event: LiveEvent) -> Result<()> {
        match event.cmd.as_str() {
            "LIVE_OPEN_PLATFORM_DM" => {
                let raw_message = match event.field_str(&["msg"]) {
                    Some(msg) => msg,
                    None => return Ok(()),
                };

                let trimmed = raw_message.trim();
                if trimmed.is_empty() {
                    return Ok(());
                }

                let sender = event
                    .field_str(&["uname"])
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| "匿名用户".to_string());

                info!(
                    target: "bilibili::live",
                    %sender,
                    message = trimmed,
                    "收到直播弹幕，准备执行意图判断"
                );

                match self.handle(trimmed).await {
                    Ok(outcome) => {
                        info!(
                            target: "bilibili::live",
                            %sender,
                            message = trimmed,
                            "直播消息触发自动执行"
                        );
                        let metadata = json!({
                            "sender": sender,
                            "message": trimmed,
                        });
                        self.broadcast_outcome("live", Some(metadata), &outcome);
                    }
                    Err(err) => {
                        warn!(
                            target: "bilibili::live",
                            error = ?err,
                            %sender,
                            message = trimmed,
                            "直播弹幕处理失败"
                        );
                        self.broadcast_error("live", &format!("直播消息处理失败: {err}"));
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }

    pub async fn handle(&mut self, input: &str) -> Result<ExecutionOutcome> {
        let intent = self.classifier.classify(input).await?;
        info!(target: "agent_controller", %intent, "收到用户请求");

        match intent {
            Intent::Conversation | Intent::Unknown => {
                let agent = self
                    .conversation
                    .as_mut()
                    .ok_or_else(|| AgentError::MissingConfig("providers.conversation (聊天)"))?;
                let response = agent.chat(input).await?;
                Ok(ExecutionOutcome::Conversation { response })
            }
            Intent::ImageGeneration => {
                let generator = self
                    .image
                    .as_ref()
                    .ok_or_else(|| AgentError::MissingConfig("providers.image (绘画生成)"))?;
                let artifact = generator.generate(input, None).await?;
                let path = self
                    .writer
                    .persist(Intent::ImageGeneration, &artifact)
                    .await?;
                Ok(ExecutionOutcome::Artifact {
                    intent: Intent::ImageGeneration,
                    path,
                    description: artifact.summary.clone(),
                })
            }
            Intent::MusicGeneration => {
                let generator = self
                    .music
                    .as_ref()
                    .ok_or_else(|| AgentError::MissingConfig("providers.music (音乐生成)"))?;
                let artifact = generator.compose(input).await?;
                let path = self
                    .writer
                    .persist(Intent::MusicGeneration, &artifact)
                    .await?;
                Ok(ExecutionOutcome::Artifact {
                    intent: Intent::MusicGeneration,
                    path,
                    description: artifact.summary.clone(),
                })
            }
            Intent::VideoGeneration => {
                let generator = self
                    .video
                    .as_ref()
                    .ok_or_else(|| AgentError::MissingConfig("providers.video (视频生成)"))?;
                let artifact = generator.generate(input).await?;
                let path = self
                    .writer
                    .persist(Intent::VideoGeneration, &artifact)
                    .await?;
                Ok(ExecutionOutcome::Artifact {
                    intent: Intent::VideoGeneration,
                    path,
                    description: artifact.summary.clone(),
                })
            }
            Intent::Help => Ok(ExecutionOutcome::Help {
                message: self.help_message(),
            }),
        }
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        if let Some(manager) = self.live.as_mut() {
            let _ = manager.stop().await?;
        }
        Ok(())
    }

    pub fn help_message(&self) -> String {
        let mut lines = vec![
            "欢迎使用 Vutber Agent!".to_string(),
            String::new(),
            "我可以帮你处理以下任务:".to_string(),
        ];

        for (intent, enabled) in self.capabilities_overview() {
            let status = if enabled {
                "✅ 已启用"
            } else {
                "⚠️ 待配置"
            };
            let description = match intent {
                Intent::Conversation => "自由对话与问答",
                Intent::ImageGeneration => "创建插画或设计草图 (OPENAI_API_KEY)",
                Intent::MusicGeneration => "根据提示生成音乐 (HYPERBOLIC_API_KEY)",
                Intent::VideoGeneration => "调用自定义视频服务生成短片 (VIDEO_API_ENDPOINT)",
                Intent::Help | Intent::Unknown => "帮助信息",
            };
            lines.push(format!("- {}: {}", description, status));
        }

        lines.push(String::new());
        lines.push("示例:".to_string());
        lines.push("- ‘帮我写一段旅行 vlog 脚本’".to_string());
        lines.push("- ‘把这段文案读出来：……’".to_string());
        lines.push("- ‘画一张赛博朋克风格的城市夜景’".to_string());
        lines.push("- ‘写一段轻快的 lofi 风格背景音乐’".to_string());
        lines.push("- ‘制作一个 10 秒的启动动画蓝图’".to_string());

        lines.push(String::new());
        lines.push("通过 WebSocket 发送 JSON 消息即可与我交互。例如：".to_string());
        lines.push(r#"- {"action":"command","input":"写一段旅行 vlog 脚本"}"#.to_string());
        lines.push(r#"- {"action":"command","input":"画一只赛博朋克猫娘"}"#.to_string());
        lines.push(r#"- {"action":"command","input":"把下面一段文字读出来……"}"#.to_string());

        if self.live.is_some() {
            lines.push(String::new());
            lines.push("直播相关命令：".to_string());
            lines.push(r#"- {"action":"live_start"} — 使用配置的身份码启动监听"#.to_string());
            lines.push(r#"- {"action":"live_stop"} — 停止监听"#.to_string());
            lines.push(r#"- {"action":"live_status"} — 查询当前状态"#.to_string());
        }

        lines.join("\n")
    }

    fn broadcast_outcome(&self, origin: &str, metadata: Option<Value>, outcome: &ExecutionOutcome) {
        let (event, mut payload) = outcome.as_event_payload();
        attach_context(&mut payload, origin, metadata);
        self.broadcast(event, payload);
    }

    fn broadcast_error(&self, origin: &str, message: &str) {
        self.broadcast(
            "agent.error",
            json!({
                "origin": origin,
                "message": message,
            }),
        );
    }

    fn broadcast(&self, event: &str, payload: serde_json::Value) {
        if let Some(broadcaster) = &self.broadcaster {
            crate::sse::broadcast_json(broadcaster, event, payload);
        }
    }
}

fn attach_context(payload: &mut Value, origin: &str, metadata: Option<Value>) {
    if let Value::Object(map) = payload {
        map.insert("origin".to_string(), json!(origin));
        if let Some(metadata) = metadata {
            map.insert("context".to_string(), metadata);
        }
    }
}
