use crate::{
    config::ZhipuConfig,
    errors::Result,
    providers::zhipu::ZhipuCompletionModel,
};
use rig::{
    agent::Agent,
    client::CompletionClient,
    completion::{Chat, Message, request::CompletionModel},
    one_or_many::OneOrMany,
    providers::openai,
};

type OpenAiCompletionModel = <openai::Client as CompletionClient>::CompletionModel;

const MAX_HISTORY_MESSAGES: usize = 24;

pub struct ConversationAgent {
    backend: ConversationBackend,
    history: Vec<ConversationMessage>,
}

enum ConversationBackend {
    OpenAi(OpenAiConversation),
    Zhipu(ZhipuConversation),
}

struct OpenAiConversation {
    agent: Agent<OpenAiCompletionModel>,
}

struct ZhipuConversation {
    model: ZhipuCompletionModel,
    preamble: String,
}

#[derive(Clone)]
struct ConversationMessage {
    role: ConversationRole,
    content: String,
}

#[derive(Clone, Copy)]
enum ConversationRole {
    User,
    Assistant,
}

impl ConversationMessage {
    fn user(content: &str) -> Self {
        Self {
            role: ConversationRole::User,
            content: content.to_string(),
        }
    }

    fn assistant(content: &str) -> Self {
        Self {
            role: ConversationRole::Assistant,
            content: content.to_string(),
        }
    }

    fn to_openai(&self) -> Message {
        match self.role {
            ConversationRole::User => Message::user(&self.content),
            ConversationRole::Assistant => Message::assistant(&self.content),
        }
    }
}

impl ConversationAgent {
    pub fn with_openai(agent: Agent<OpenAiCompletionModel>) -> Self {
        Self {
            backend: ConversationBackend::OpenAi(OpenAiConversation { agent }),
            history: Vec::new(),
        }
    }

    // 使用 rig CompletionModel API
    pub fn with_zhipu(config: &ZhipuConfig, model_override: Option<&str>) -> Result<Self> {
        let model = ZhipuCompletionModel::from_config(config, model_override)?;
        Ok(Self {
            backend: ConversationBackend::Zhipu(ZhipuConversation {
                model,
                preamble: config.agent_preamble.clone(),
            }),
            history: Vec::new(),
        })
    }

    fn trim_history(&mut self) {
        if self.history.len() > MAX_HISTORY_MESSAGES {
            let overflow = self.history.len() - MAX_HISTORY_MESSAGES;
            self.history.drain(0..overflow);
        }
    }

    pub async fn chat(&mut self, user_input: &str) -> Result<String> {
        let history_snapshot = self.history.clone();

        let response = match &mut self.backend {
            ConversationBackend::OpenAi(openai) => {
                let formatted_history: Vec<Message> =
                    history_snapshot.iter().map(|msg| msg.to_openai()).collect();
                openai.agent.chat(user_input, formatted_history).await?
            }
            ConversationBackend::Zhipu(zhipu) => {
                // 构建聊天历史消息
                let mut messages = vec![];
                
                // 添加系统提示词作为用户消息
                if !zhipu.preamble.is_empty() {
                    messages.push(Message::User {
                        content: OneOrMany::one(rig::completion::message::UserContent::Text(
                            rig::completion::message::Text {
                                text: zhipu.preamble.clone(),
                            }
                        )),
                    });
                }
                
                // 添加历史消息
                for msg in &history_snapshot {
                    messages.push(msg.to_openai());
                }
                
                // 添加当前用户输入
                messages.push(Message::user(user_input));
                
                // 构建请求
                let request = zhipu.model
                    .completion_request("")
                    .messages(messages)
                    .build();
                
                // 调用模型
                let response = zhipu.model.completion(request).await
                    .map_err(|e| crate::errors::AgentError::Unsupported(format!("智谱AI调用失败: {}", e)))?;
                
                // 提取响应文本
                response.choice.iter()
                    .filter_map(|content| match content {
                        rig::completion::message::AssistantContent::Text(t) => Some(t.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        };

        self.history.push(ConversationMessage::user(user_input));
        self.history.push(ConversationMessage::assistant(&response));
        self.trim_history();

        Ok(response)
    }
}
