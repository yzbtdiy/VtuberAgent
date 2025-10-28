use std::{
    env, fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, anyhow};
use rig::providers::openai;
use serde::Deserialize;

const DEFAULT_CONFIG_PATH: &str = "config/app_config.toml";
const DEFAULT_PREAMBLE: &str = "You are Vutber, a multi-modal creative AI who can chat, narrate, sing, paint and storyboard videos.";
const DEFAULT_ZHIPU_API_URL: &str = "https://open.bigmodel.cn/api/paas/v4/chat/completions";

#[derive(Clone, Debug)]
pub struct OpenAiConfig {
    pub api_key: String,
    pub base_url: Option<String>,
    pub chat_model: String,
    pub agent_preamble: String,
    pub image_model: String,
}

#[derive(Clone, Debug)]
pub struct HyperbolicConfig {
    pub api_key: String,
    pub language: String,
    pub voice: String,
}

#[derive(Clone, Debug)]
pub struct VideoConfig {
    pub endpoint: String,
    pub api_key: Option<String>,
    pub format: String,
    pub max_duration_seconds: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub openai: Option<OpenAiConfig>,
    pub hyperbolic: Option<HyperbolicConfig>,
    pub video: Option<VideoConfig>,
    pub zhipu: Option<ZhipuConfig>,
    pub bilibili_live: Option<BilibiliLiveConfig>,
    pub providers: CapabilityProviders,
    pub artifacts_dir: PathBuf,
    pub sse: SseConfig,
}

impl AppConfig {
    pub fn load() -> anyhow::Result<Self> {
        let config_path =
            env::var("APP_CONFIG_PATH").unwrap_or_else(|_| DEFAULT_CONFIG_PATH.to_string());
        let config_path = Path::new(&config_path);

        let contents = fs::read_to_string(config_path)
            .with_context(|| format!("读取配置文件 {:?} 失败", config_path))?;

        let file_config: FileConfig = toml::from_str(&contents)
            .with_context(|| format!("解析配置文件 {:?} 失败", config_path))?;

        let artifacts_dir = if let Some(dir) = &file_config.artifacts_dir {
            PathBuf::from(dir)
        } else if let Ok(dir) = env::var("ARTIFACTS_DIR") {
            PathBuf::from(dir)
        } else {
            env::current_dir()?.join("artifacts")
        };

        let openai = file_config.openai.and_then(|section| section.into_domain());
        let hyperbolic = file_config
            .hyperbolic
            .and_then(|section| section.into_domain());
        let video = file_config.video.and_then(|section| section.into_domain());
        let zhipu = file_config.zhipu.and_then(|section| section.into_domain());
        let bilibili_live = file_config
            .live
            .and_then(|section| section.bilibili)
            .and_then(|section| section.into_domain());

        let providers = CapabilityProviders::from_file(
            file_config.providers,
            openai.as_ref(),
            zhipu.as_ref(),
            hyperbolic.as_ref(),
            video.as_ref(),
        );

        let sse = file_config
            .sse
            .map(|section| section.into_domain())
            .transpose()?;

        let sse = sse.ok_or_else(|| {
            anyhow!(
                "请在 config/app_config.toml 中配置 [sse] 段 (access_key, secret_key, bind_addr)"
            )
        })?;

        Ok(Self {
            openai,
            hyperbolic,
            video,
            zhipu,
            bilibili_live,
            providers,
            artifacts_dir,
            sse,
        })
    }
}

#[derive(Debug, Deserialize, Default)]
struct FileConfig {
    #[serde(default)]
    artifacts_dir: Option<String>,
    #[serde(default)]
    openai: Option<FileOpenAiConfig>,
    #[serde(default)]
    hyperbolic: Option<FileHyperbolicConfig>,
    #[serde(default)]
    video: Option<FileVideoConfig>,
    #[serde(default)]
    zhipu: Option<FileZhipuConfig>,
    #[serde(default)]
    live: Option<FileLiveConfig>,
    #[serde(default)]
    providers: Option<FileCapabilityProviders>,
    #[serde(default)]
    sse: Option<FileSseConfig>,
}

#[derive(Debug, Deserialize, Default)]
struct FileOpenAiConfig {
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    chat_model: Option<String>,
    #[serde(default)]
    agent_preamble: Option<String>,
    #[serde(default)]
    image_model: Option<String>,
}

impl FileOpenAiConfig {
    fn into_domain(self) -> Option<OpenAiConfig> {
        let api_key = self.api_key?;

        Some(OpenAiConfig {
            api_key,
            base_url: self.base_url,
            chat_model: self.chat_model.unwrap_or_else(|| "gpt-4o-mini".to_string()),
            agent_preamble: self
                .agent_preamble
                .unwrap_or_else(|| DEFAULT_PREAMBLE.to_string()),
            image_model: self
                .image_model
                .unwrap_or_else(|| openai::DALL_E_3.to_string()),
        })
    }
}

#[derive(Debug, Deserialize, Default)]
struct FileHyperbolicConfig {
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    voice: Option<String>,
}

impl FileHyperbolicConfig {
    fn into_domain(self) -> Option<HyperbolicConfig> {
        let api_key = self.api_key?;

        Some(HyperbolicConfig {
            api_key,
            language: self.language.unwrap_or_else(|| "EN".to_string()),
            voice: self.voice.unwrap_or_else(|| "EN-US".to_string()),
        })
    }
}

#[derive(Debug, Deserialize, Default)]
struct FileVideoConfig {
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    max_duration_seconds: Option<u32>,
}

impl FileVideoConfig {
    fn into_domain(self) -> Option<VideoConfig> {
        let endpoint = self.endpoint?;

        Some(VideoConfig {
            endpoint,
            api_key: self.api_key,
            format: self.format.unwrap_or_else(|| "mp4".to_string()),
            max_duration_seconds: self.max_duration_seconds,
        })
    }
}

#[derive(Clone, Debug)]
pub struct ZhipuConfig {
    pub api_key: String,
    pub chat_model: String,
    /// Agent 系统提示词，用于对话场景
    pub agent_preamble: String,
    pub api_url: String,
}

#[derive(Clone, Debug)]
pub struct BilibiliLiveConfig {
    pub access_key: String,
    pub access_secret: String,
    pub app_id: i64,
    pub id_code: Option<String>,
    pub host: Option<String>,
    pub heartbeat_interval_seconds: u64,
}

#[derive(Debug, Deserialize, Default)]
struct FileLiveConfig {
    #[serde(default)]
    bilibili: Option<FileBilibiliLiveConfig>,
}

#[derive(Debug, Deserialize, Default)]
struct FileZhipuConfig {
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    chat_model: Option<String>,
    #[serde(default)]
    agent_preamble: Option<String>,
    #[serde(default)]
    api_url: Option<String>,
}

impl FileZhipuConfig {
    fn into_domain(self) -> Option<ZhipuConfig> {
        let api_key = self.api_key?;

        Some(ZhipuConfig {
            api_key,
            chat_model: self.chat_model.unwrap_or_else(|| "glm-4-flash".to_string()),
            agent_preamble: self
                .agent_preamble
                .unwrap_or_else(|| DEFAULT_PREAMBLE.to_string()),
            api_url: self
                .api_url
                .unwrap_or_else(|| DEFAULT_ZHIPU_API_URL.to_string()),
        })
    }
}

const DEFAULT_BILIBILI_HEARTBEAT_SECONDS: u64 = 20;

#[derive(Debug, Deserialize, Default)]
struct FileBilibiliLiveConfig {
    #[serde(default)]
    access_key: Option<String>,
    #[serde(default)]
    access_secret: Option<String>,
    #[serde(default)]
    app_id: Option<i64>,
    #[serde(default)]
    id_code: Option<String>,
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    heartbeat_interval_seconds: Option<u64>,
}

impl FileBilibiliLiveConfig {
    fn into_domain(self) -> Option<BilibiliLiveConfig> {
        let access_key = self.access_key?;
        let access_secret = self.access_secret?;
        let app_id = self.app_id?;
        let heartbeat = self
            .heartbeat_interval_seconds
            .unwrap_or(DEFAULT_BILIBILI_HEARTBEAT_SECONDS)
            .max(5);

        Some(BilibiliLiveConfig {
            access_key,
            access_secret,
            app_id,
            id_code: self.id_code,
            host: self.host,
            heartbeat_interval_seconds: heartbeat,
        })
    }
}

#[derive(Clone, Debug)]
pub struct SseConfig {
    pub access_key: String,
    pub secret_key: String,
    pub bind_addr: SocketAddr,
    pub signature_ttl: Duration,
}

#[derive(Debug, Deserialize, Default)]
struct FileSseConfig {
    #[serde(default)]
    access_key: Option<String>,
    #[serde(default)]
    secret_key: Option<String>,
    #[serde(default)]
    bind_addr: Option<String>,
    #[serde(default)]
    signature_ttl_seconds: Option<u64>,
}

impl FileSseConfig {
    fn into_domain(self) -> anyhow::Result<SseConfig> {
        let access_key = self
            .access_key
            .ok_or_else(|| anyhow!("sse.access_key 未配置"))?;
        let secret_key = self
            .secret_key
            .ok_or_else(|| anyhow!("sse.secret_key 未配置"))?;

        let bind_addr_str = self
            .bind_addr
            .unwrap_or_else(|| "127.0.0.1:9000".to_string());
        let bind_addr = bind_addr_str
            .parse::<SocketAddr>()
            .with_context(|| format!("解析 sse.bind_addr 失败: {}", bind_addr_str))?;

        let ttl_seconds = self.signature_ttl_seconds.unwrap_or(300).max(30);

        Ok(SseConfig {
            access_key,
            secret_key,
            bind_addr,
            signature_ttl: Duration::from_secs(ttl_seconds),
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct CapabilityProviders {
    pub intent: Option<CapabilityRoute>,
    pub conversation: Option<CapabilityRoute>,
    pub image: Option<CapabilityRoute>,
    pub music: Option<CapabilityRoute>,
    pub video: Option<CapabilityRoute>,
}

impl CapabilityProviders {
    fn from_file(
        file: Option<FileCapabilityProviders>,
        openai: Option<&OpenAiConfig>,
        zhipu: Option<&ZhipuConfig>,
        hyperbolic: Option<&HyperbolicConfig>,
        video: Option<&VideoConfig>,
    ) -> Self {
        let file = file.unwrap_or_default();

        Self {
            intent: file
                .intent
                .and_then(FileCapabilityRoute::into_domain)
                .or_else(|| Self::default_intent(openai, zhipu)),
            conversation: file
                .conversation
                .and_then(FileCapabilityRoute::into_domain)
                .or_else(|| Self::default_conversation(openai, zhipu)),
            image: file
                .image
                .and_then(FileCapabilityRoute::into_domain)
                .or_else(|| Self::default_image(openai)),
            music: file
                .music
                .and_then(FileCapabilityRoute::into_domain)
                .or_else(|| Self::default_music(hyperbolic)),
            video: file
                .video
                .and_then(FileCapabilityRoute::into_domain)
                .or_else(|| Self::default_video(video)),
        }
    }

    fn default_intent(
        openai: Option<&OpenAiConfig>,
        zhipu: Option<&ZhipuConfig>,
    ) -> Option<CapabilityRoute> {
        if let Some(cfg) = openai {
            return Some(CapabilityRoute::new("openai", Some(cfg.chat_model.clone())));
        }
        zhipu.map(|cfg| CapabilityRoute::new("zhipu", Some(cfg.chat_model.clone())))
    }

    fn default_conversation(
        openai: Option<&OpenAiConfig>,
        zhipu: Option<&ZhipuConfig>,
    ) -> Option<CapabilityRoute> {
        if let Some(cfg) = openai {
            return Some(CapabilityRoute::new("openai", Some(cfg.chat_model.clone())));
        }
        zhipu.map(|cfg| CapabilityRoute::new("zhipu", Some(cfg.chat_model.clone())))
    }

    fn default_image(openai: Option<&OpenAiConfig>) -> Option<CapabilityRoute> {
        openai.map(|cfg| CapabilityRoute::new("openai", Some(cfg.image_model.clone())))
    }

    fn default_music(hyperbolic: Option<&HyperbolicConfig>) -> Option<CapabilityRoute> {
        hyperbolic.map(|cfg| CapabilityRoute::new("hyperbolic", Some(cfg.language.clone())))
    }

    fn default_video(video: Option<&VideoConfig>) -> Option<CapabilityRoute> {
        video.map(|cfg| CapabilityRoute::new("custom", Some(cfg.format.clone())))
    }
}

#[derive(Clone, Debug)]
pub struct CapabilityRoute {
    pub provider: String,
    pub model: Option<String>,
}

impl CapabilityRoute {
    fn new(provider: impl Into<String>, model: Option<String>) -> Self {
        let provider = provider.into();
        Self {
            provider: provider.trim().to_lowercase(),
            model,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
struct FileCapabilityProviders {
    #[serde(default)]
    intent: Option<FileCapabilityRoute>,
    #[serde(default)]
    conversation: Option<FileCapabilityRoute>,
    #[serde(default)]
    image: Option<FileCapabilityRoute>,
    #[serde(default)]
    music: Option<FileCapabilityRoute>,
    #[serde(default)]
    video: Option<FileCapabilityRoute>,
}

#[derive(Debug, Deserialize, Default)]
struct FileCapabilityRoute {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

impl FileCapabilityRoute {
    fn into_domain(self) -> Option<CapabilityRoute> {
        let provider = self.provider?;
        Some(CapabilityRoute::new(provider, self.model))
    }
}
