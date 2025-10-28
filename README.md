# VutberAgent

一个基于 [rig](https://github.com/0xPlaygrounds/rig) 构建的多模态 Agent，支持意图识别、文本对话、图像生成、音乐生成与可扩展的视频生成。

## 功能概览

| 能力 | 描述 | 默认提供方 | 配置字段（`config/app_config.toml`） |
| --- | --- | --- | --- |
| 用户意图识别 | 基于 LLM 的多能力路由，自动选择后续动作 | OpenAI / 智谱 GLM | `providers.intent`；所选提供方的密钥与模型配置（如 `openai.*` 或 `zhipu.*`） |
| 文本对话 | 持续对话与问答，自动维护上下文 | OpenAI Responses API / 智谱 GLM | `providers.conversation`；所选提供方的密钥与模型配置 |
| 图像生成 | 根据提示生成 PNG 图片 | OpenAI DALL·E 系列 | `providers.image`；并配置 `openai.*` 图像模型 |
| 音乐生成 | 使用 Hyperbolic 音频生成 API 输出 MP3 乐段 | Hyperbolic Inference | `providers.music`；并配置 `hyperbolic.*` 语言与音色 |
| 视频生成 | 调用自定义视频服务（Base64 / JSON / 二进制流）并保存结果 | 自定义 | `providers.video`；并配置 `video.*` 端点、密钥与格式 |

> 未配置的能力会自动提示对应的配置字段，不会导致程序崩溃。

## 快速开始

1. **准备 Rust 环境**

   ```bash
   # Windows (PowerShell)
   rustup default stable
   cargo --version
   ```

2. **创建配置文件**

   ```pwsh
   # 复制示例文件并填写真实参数
   Copy-Item config/app_config.example.toml config/app_config.toml
   # 编辑 config/app_config.toml，填入各服务的 api_key / 模型名称等
   ```

   可通过以下字段定制：

   - `openai.*`：聊天、意图识别、图像生成所需的模型、密钥与可选 `base_url`（用于 OpenAI 兼容接口）。
   - `hyperbolic.*`：音乐生成所需的 Hyperbolic API 信息。
   - `zhipu.*`：智谱 GLM 对话所需的密钥、模型与可选的 API URL，可在对话或意图识别中按需启用。
   - `providers.*`：为各项能力选择具体的提供方与模型名，可显式禁用或切换不同供应商。
   - `video.*`：自定义视频生成服务的调用参数。
   - `sse.*`：SSE 服务的 `access_key`、`secret_key`、可选的 `bind_addr`（默认 `127.0.0.1:9000`）与 `signature_ttl_seconds`。
   - `artifacts_dir`：可选，指定生成文件的输出目录。

3. **启动 SSE 服务**

   ```pwsh
   cargo run
   ```

         启动前请在 `config/app_config.toml` 的 `[sse]` 段填写 `access_key` 与 `secret_key`（可选调整 `bind_addr`、`signature_ttl_seconds`）。默认会监听 `127.0.0.1:9000`。鉴权采用 HMAC-SHA256 签名：客户端需附加查询参数 `access_key`、`timestamp`（秒）、`nonce`（16 字节随机值）和 `signature`（对 `access_key:timestamp:nonce` 以 `secret_key` 计算的签名）。示例测试页 `web/sse-test.html` 会在连接前自动生成这些参数。完整的接口说明见 `docs/sse-api.md`。

         **SSE 架构**：
         - **事件流（GET /events）**：使用 `EventSource` 接收服务器推送的事件（如 `agent.conversation`、`agent.artifact`、`live.started` 等）
         - **命令提交（POST /command）**：通过 `fetch()` 发送 JSON 格式的命令

         消息格式示例：

   - `{"action":"command","input":"写一段旅行 vlog 脚本"}`
   - `{"action":"command","input":"帮我写一个直播开场白"}`
   - `{"action":"live_start"}` / `{"action":"live_stop"}` / `{"action":"live_status"}`

   服务器会向所有订阅端广播结构化事件，前端按需渲染即可。

4. **输出位置**

   - 所有二进制产物（音频/图像/视频）会保存至 `artifacts/` 目录，并生成同名 `*.meta.json` 元信息。
   - 命令行中会提示生成文件的绝对路径。

## 项目结构

```
├── Cargo.toml
├── src
│   ├── main.rs                # 应用入口（初始化 SSE 服务与调度器）
│   ├── config.rs              # 配置文件读取与转换
│   ├── errors.rs              # 统一错误类型
│   ├── orchestrator.rs        # 能力编排控制器
│   ├── capabilities/          # 各种原子能力
│   │   ├── conversation.rs
│   │   ├── image.rs
│   │   ├── music.rs
│   │   └── video.rs
│   ├── intent/                # 意图识别
│   │   └── classifier.rs
│   ├── sse.rs                 # 基于 Axum 的 SSE 服务（事件流 + 命令接口）
│   ├── live/                  # B站直播监听（使用 WebSocket）
│   └── util/                  # 基础工具
│       └── writer.rs
└── README.md
```

## 可扩展性

- **视频服务**：默认假定返回 JSON 字段 `video_base64` 或 `video_url`。如需适配其它协议，可修改 `src/capabilities/video.rs`。
- **意图路由**：`IntentClassifier` 支持 OpenAI LLM 分类，同时提供关键字回退策略，可接入自定义模型。
- **RAG / 工具调用**：可在 `AgentController` 中注入更多 `capabilities::*` 模块或度量逻辑。

## 常见问题

- 未在 `config/app_config.toml` 中填写某项服务时，CLI 会提示缺失的配置字段并给出帮助信息。
- 视频服务返回二进制流时，会根据 `Content-Type` 自动保存；若返回 JSON，请确保携带 `video_base64` 或可访问的 `video_url`。
- 如需更换配置文件路径，可设置 `APP_CONFIG_PATH` 环境变量；若想覆盖输出目录，可设置 `ARTIFACTS_DIR`。

## 开发与测试

```pwsh
cargo fmt
cargo check
```

如需引入新的能力，建议：

1. 在 `capabilities/` 目录中创建模块，实现 `BinaryArtifact` 或文本输出。
2. 在 `AgentController` 中注册能力，并在 `Intent` 枚举中增加路由。
3. 针对输出产物更新 `ArtifactWriter` 以存储或回传。

---

> ⚠️ 使用真实 API Key 前，请确认网络环境、费用策略与数据合规要求。README 所示变量仅为示例。
