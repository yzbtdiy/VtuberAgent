# VTuber API - Rust 实现

基于 Rust 和 Rig AI 框架构建的高性能 VTuber 弹幕处理系统。

## 项目概述

这是一个高性能的 VTuber 弹幕处理系统的 Rust 实现，使用最新的 Rig 0.18.1 框架进行 AI 操作。它提供实时 WebSocket 通信、基于签名的身份验证，以及具有图像生成和文本转语音功能的智能弹幕处理。该系统设计用于与各种 AI API 端点配合工作，并支持不同 AI 提供商的灵活配置。

## 功能特性

- **纯 WebSocket 架构**：实时双向通信
- **Rig AI 框架 0.18.1**：利用最新的 Rig 版本进行智能多代理处理
- **灵活的 AI API 支持**：可配置的基础 URL 支持多种 AI 端点
- **强制身份验证**：基于 HMAC-SHA256 签名的身份验证
- **意图分析**：自动分类弹幕内容
- **多模态响应**：文本、音频和图像生成
- **实时进度更新**：处理过程中的实时状态报告
- **高性能**：基于 Rust 2024 edition 构建，保证速度和可靠性
- **JSON 配置管理**：结构化的 config.json 配置文件，支持复杂配置
- **图像 URL 响应**：返回图像 URL 而非二进制数据，提高兼容性

## 系统架构

### 核心组件

1. **WebSocket 服务器** (`src/api/websocket_server.rs`)：基于 Axum 的 WebSocket 服务器
2. **身份验证服务** (`src/auth/mod.rs`)：HMAC-SHA256 签名验证
3. **AI 代理** (`src/agents/mod.rs`)：基于 Rig 的意图分析和响应生成
4. **工具集** (`src/tools/mod.rs`)：图像生成和 TTS 集成
5. **工作流引擎** (`src/workflows/mod.rs`)：编排完整的处理流水线
6. **客户端管理器** (`src/services/mod.rs`)：管理 WebSocket 连接和身份验证

### AI 处理流水线

1. **意图分析**：使用 Rig 代理分类弹幕意图
2. **响应生成**：针对不同内容类型的专门代理
3. **图像生成**：AI 驱动的绘画功能用于艺术请求
4. **音频合成**：文本转语音功能用于语音响应
5. **进度报告**：整个处理过程的实时更新

## 快速开始

### 系统要求

- Rust 1.70+ 和 Cargo (推荐使用 2024 edition)
- 兼容 OpenAI API 格式的 AI API 端点
- AI 服务的 API 密钥
- （可选）用于增强 RAG 功能的向量数据库

### 安装步骤

```bash
# 克隆仓库
git clone <repository-url>
cd VtuberAPI

# 构建项目
cargo build

# 复制配置文件
cp config.example.json config.json
# 编辑 config.json 文件配置您的 API 设置
```

### 配置说明

编辑 `config.json` 文件进行配置：

```json
{
  "server": {
    "host": "localhost",
    "port": 8000
  },
  "auth": {
    "secret_key": "your_secret_key_here_change_in_production",
    "valid_api_keys": [
      "your-api-key-1",
      "your-api-key-2"
    ],
    "timestamp_tolerance": 300
  },
  "openai": {
    "api_key": "your_openai_api_key_here",
    "base_url": "https://api.openai.com/v1",
    "model": "gpt-3.5-turbo",
    "embedding_model": "text-embedding-3-small",
    "tts_model": "tts-1",
    "tts_voice": "alloy"
  },
  "processing": {
    "max_danmaku_length": 100,
    "response_timeout": 30,
    "max_execution_time": 120
  },
  "logging": {
    "rust_log": "info",
    "otel_sdk_disabled": true,
    "crewai_telemetry_disabled": true
  }
}
```

**注意**：此系统使用结构化的 JSON 配置文件而非环境变量，以实现更可靠和灵活的配置管理。

### 运行程序

```bash
# 开发模式
cargo run

# 生产构建和运行
cargo build --release
./target/release/VtuberAPI

# Windows PowerShell
.\target\release\VtuberAPI.exe
```

## API 参考

### WebSocket 端点

- `ws://localhost:8000/ws` - 主要 WebSocket 端点
- `GET /health` - 健康检查
- `GET /stats` - 连接统计

### 消息类型

#### 身份验证
```json
{
  "type": "auth",
  "auth_data": {
    "type": "signature",
    "user_id": "user123",
    "api_key": "your-api-key",
    "timestamp": "2024-01-01T00:00:00Z",
    "nonce": "random-string",
    "signature": "hmac-sha256-signature"
  }
}
```

#### 弹幕处理
```json
{
  "type": "danmaku",
  "content": "画一只可爱的小猫咪",
  "user_id": "user123",
  "timestamp": "2024-01-01T00:00:00Z"
}
```

#### 进度更新
```json
{
  "type": "progress",
  "stage": "image_generation_start",
  "message": "🎨 正在为您创作图片，请稍等片刻...",
  "image_prompt": "可爱的小猫咪"
}
```

#### 带图像 URL 的响应
```json
{
  "type": "danmaku_response",
  "content": "已为您创作了一幅可爱的小猫咪图片！",
  "image_url": "https://your-api.com/generated-image-url",
  "intent": "image_generation",
  "user_id": "user123",
  "timestamp": "2024-01-01T00:00:00Z"
}
```

## 开发指南

### 项目结构

```
src/
├── main.rs              # 应用程序入口点
├── config/              # 配置管理
├── models/              # 数据结构和类型
├── auth/                # 身份验证服务
├── agents/              # Rig AI 代理
├── tools/               # 外部服务集成
├── workflows/           # 处理编排
├── services/            # 客户端和连接管理
└── api/                 # WebSocket 服务器和端点
```

### 核心设计模式

1. **Rig 0.18.1 集成**：使用最新的 Rig 框架进行代理和完成模式
2. **灵活的 AI API 支持**：为不同 AI 提供商提供可配置的基础 URL
3. **JSON 配置管理**：结构化的 config.json 文件，支持复杂配置
4. **异步/等待**：使用 Tokio 运行时的完全异步
5. **类型安全**：利用 Rust 的类型系统保证可靠性
6. **错误处理**：使用 `anyhow` 进行全面的错误传播
7. **模块化架构**：清晰的关注点分离
8. **基于 URL 的媒体**：返回 URL 而非二进制数据以提高性能

### 添加新功能

1. **新意图类型**：扩展 `IntentType` 枚举并添加相应的代理
2. **附加工具**：在 `src/tools/` 中实现新工具
3. **增强身份验证**：扩展 `AuthService` 以支持新的认证方法
4. **向量存储**：为 RAG 功能添加 Rig 向量存储集成

## 测试

```bash
# 运行所有测试
cargo test

# 带输出运行
cargo test -- --nocapture

# 检查代码质量
cargo check
cargo clippy

# 格式化代码
cargo fmt
```

### WebSocket 测试

使用包含的 `websocket_test.html` 文件测试 WebSocket 连接：

1. 在 Web 浏览器中打开 `websocket_test.html`
2. 配置您的身份验证凭据
3. 测试身份验证和弹幕处理

## 性能特点

- **内存高效**：Rust 的零成本抽象
- **并发处理**：Tokio 异步运行时
- **连接池**：高效的 HTTP 客户端重用
- **流式响应**：实时数据处理

## 安全性

- **强制身份验证**：不允许匿名访问
- **HMAC 签名**：加密安全的身份验证
- **时间戳验证**：防止重放攻击
- **输入验证**：全面的请求验证

## 监控

服务器提供内置监控端点：

- `/health` 健康检查
- `/stats` 连接统计
- 使用 `tracing` 的结构化日志

## 生产部署

### 环境设置

1. 为生产环境设置强 `secret_key`
2. 为您的授权客户端配置 `valid_api_keys`
3. 设置您的 AI API `base_url` 和凭据
4. 设置适当的 `rust_log` 级别（例如，`"info"`）
5. 考虑为生产部署设置反向代理
6. 确保您的 AI API 支持所需的端点

## 核心依赖

```toml
# 核心框架
rig-core = "0.18.1"          # 最新的 AI 代理框架

# 异步运行时
tokio = "1.34.0"             # 具有完整功能的异步运行时

# Web 框架
axum = "0.8"                 # 支持 WebSocket 的现代 Web 框架
tokio-tungstenite = "0.27"   # WebSocket 实现

# 序列化和错误处理
serde = "1.0"                # 序列化框架
serde_json = "1.0"           # JSON 支持
anyhow = "1.0.75"            # 错误处理
thiserror = "2.0"            # 错误派生宏

# 配置管理
config = "0.15.14"           # 结构化配置文件支持

# 加密和实用程序
hmac = "0.12"                # HMAC 身份验证
sha2 = "0.10"                # SHA256 哈希
base64 = "0.22"              # Base64 编码
uuid = "1.0"                 # UUID 生成
chrono = "0.4"               # 日期和时间处理
```

## 故障排除

### 常见问题

1. **身份验证失败**：检查签名生成和时间戳验证
2. **AI API 连接问题**：验证 config.json 中的 base_url 和 api_key 配置
3. **WebSocket 连接问题**：检查防火墙和网络设置
4. **构建错误**：确保 Rust 版本 1.70+ 并运行 `cargo clean`
5. **配置文件错误**：检查 config.json 格式和必需字段

### 日志记录

通过 config.json 设置日志级别：
```json
{
  "logging": {
    "rust_log": "debug"
  }
}
```

或临时设置环境变量：
```bash
$env:RUST_LOG="debug"; cargo run           # PowerShell
RUST_LOG=debug cargo run                   # Bash/Zsh
RUST_LOG=VtuberAPI=debug cargo run        # 模块特定日志
```

## 许可证

MIT 许可证 - 详见 LICENSE 文件。

## 更新日志

### 版本 0.1.0 (2025-08-20)
- **Rig 框架升级**：使用 Rig 0.18.1 的最新功能
- **Rust 2024 Edition**：使用最新的 Rust 2024 edition
- **JSON 配置管理**：支持嵌套配置和复杂数据类型
- **改进的错误处理**：更好的配置验证和错误报告
- **灵活的日志配置**：通过配置文件管理日志级别和选项
- **WebSocket 通信**：实时双向通信支持
- **多模态响应**：文本、图像、音频的完整支持
- **HMAC-SHA256 身份验证**：带时间戳验证的安全认证

