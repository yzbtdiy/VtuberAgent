use anyhow::Result;
use rig::{completion::Prompt, providers::openai, agent::AgentBuilder, client::CompletionClient};

use crate::config::Settings;
use crate::models::IntentType;

#[derive(Clone)]
pub struct DanmakuAgents {
    openai_client: openai::Client,
    model_name: String,
}

impl DanmakuAgents {
    pub fn new(settings: &Settings) -> Self {
        // Create OpenAI client with custom base URL using ClientBuilder
        let openai_client = openai::Client::builder(&settings.openai.api_key)
            .base_url(&settings.openai.base_url)
            .build()
            .expect("Failed to build OpenAI client");

        Self { 
            openai_client,
            model_name: settings.openai.model.clone(),
        }
    }

    pub async fn analyze_intent(&self, danmaku_content: &str) -> Result<IntentType> {
        let intent_analyzer = AgentBuilder::new(
            self.openai_client.completion_model(&self.model_name)
        )
        .preamble(
            "你是一个专业的弹幕内容分析师，具有丰富的直播间互动经验。\
            你能够理解各种网络用语、表情符号和简写，准确判断观众的意图。\
            你擅长区分闲聊、请求、指令等不同类型的弹幕。\
            \
            你必须明确回答以下四种意图类型之一：\
            1. 对话聊天 - 普通问候、闲聊、提问\
            2. 唱歌请求 - 点歌、要求唱歌\
            3. 绘画请求 - 要求画画、绘制内容\
            4. 其他指令 - 特殊请求或指令\
            \
            回答格式：直接回答意图类型，例如\"绘画请求\"或\"对话聊天\"。",
        )
        .build();

        let prompt = format!(
            "请分析以下弹幕内容的意图类型：\n\n弹幕内容：{}\n\n请直接回答意图类型：",
            danmaku_content
        );

        let response = intent_analyzer.prompt(&prompt).await?;
        let intent_type = IntentType::from_str(response.trim());

        Ok(intent_type)
    }

    pub async fn generate_conversation_response(&self, danmaku_content: &str) -> Result<String> {
        let conversation_agent = AgentBuilder::new(
            self.openai_client.completion_model(&self.model_name)
        )
        .preamble(
            "你是一个可爱的虚拟主播助手，性格开朗活泼，善于与观众互动。\
            你会用亲切的语气回应观众的问候、闲聊和日常话题。\
            你了解网络文化和流行梗，能够与年轻观众产生共鸣。\
            请用简洁、自然的语言回应，保持亲切友好的语调。",
        )
        .build();

        let prompt = format!(
            "观众说：{}\n\n请以虚拟主播的身份亲切地回应这位观众：",
            danmaku_content
        );

        let response = conversation_agent.prompt(&prompt).await?;
        Ok(response)
    }

    pub async fn generate_singing_response(&self, danmaku_content: &str) -> Result<String> {
        let singing_agent = AgentBuilder::new(
            self.openai_client.completion_model(&self.model_name)
        )
        .preamble(
            "你是一个音乐专家和表演指导，熟悉各种类型的歌曲。\
            你能够根据观众的点歌请求选择合适的歌曲，并给出演唱建议。\
            你了解不同歌曲的风格、难度和适合的演唱方式。\
            请用热情友好的语调回应点歌请求。",
        )
        .build();

        let prompt = format!(
            "观众的点歌请求：{}\n\n请回应这个点歌请求，可以介绍歌曲或给出演唱建议：",
            danmaku_content
        );

        let response = singing_agent.prompt(&prompt).await?;
        Ok(response)
    }

    pub async fn generate_drawing_response(&self, danmaku_content: &str) -> Result<(String, String)> {
        let drawing_agent = AgentBuilder::new(
            self.openai_client.completion_model(&self.model_name)
        )
        .preamble(
            "你是一个专业的绘画助手和艺术指导，擅长理解绘画需求并进行创作。\
            你能够将观众的绘画请求转化为详细的绘画描述。\
            请用温暖友好的语调回应绘画请求，并提供简洁的图像描述用于AI绘画。",
        )
        .build();

        let prompt = format!(
            "观众的绘画请求：{}\n\n请完成两个任务：\
            1. 用友好的语调回应这个绘画请求\
            2. 提供一个简洁的英文图像描述（用于AI绘画）\
            \n请按以下格式回答：\
            回应：[你的友好回应]\
            图像描述：[英文图像描述]",
            danmaku_content
        );

        let response = drawing_agent.prompt(&prompt).await?;
        
        // Parse the response to extract both parts
        let parts: Vec<&str> = response.split("图像描述：").collect();
        if parts.len() == 2 {
            let response_text = parts[0].replace("回应：", "").trim().to_string();
            let image_prompt = parts[1].trim().to_string();
            Ok((response_text, image_prompt))
        } else {
            // Fallback if parsing fails
            Ok((response, format!("Drawing based on: {}", danmaku_content)))
        }
    }

    pub async fn generate_other_response(&self, danmaku_content: &str) -> Result<String> {
        let other_agent = AgentBuilder::new(
            self.openai_client.completion_model(&self.model_name)
        )
        .preamble(
            "你是一个多才多艺的虚拟主播助手，能够处理各种特殊请求和指令。\
            你总是积极响应观众的需求，用友好的态度提供帮助。\
            对于无法完成的请求，你会礼貌地说明原因并提供替代建议。",
        )
        .build();

        let prompt = format!(
            "观众的请求：{}\n\n请以虚拟主播的身份回应这个特殊请求：",
            danmaku_content
        );

        let response = other_agent.prompt(&prompt).await?;
        Ok(response)
    }
}
