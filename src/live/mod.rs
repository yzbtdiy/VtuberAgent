mod packet;

use std::{sync::Arc, time::Duration};

use crate::{
    config::BilibiliLiveConfig,
    errors::{AgentError, Result},
    sse::broadcast_json,
    util::now_in_beijing,
};
use chrono::{DateTime, FixedOffset, TimeZone};
use futures::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use md5::{Digest, Md5};
use packet::{
    BiliPacket, OP_AUTH, OP_AUTH_REPLY, OP_HEARTBEAT, OP_HEARTBEAT_REPLY, OP_SEND_EVENT,
    decode_packets, encode_packet,
};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use tokio::{
    select,
    sync::{broadcast, mpsc},
    task::JoinHandle,
    time::{self, MissedTickBehavior},
};
use tokio_stream::wrappers::BroadcastStream;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};
use uuid::Uuid;

const DEFAULT_BASE_URL: &str = "https://live-open.biliapi.com";
const DEFAULT_HEARTBEAT_INTERVAL: u64 = 20;

#[derive(Debug)]
pub struct LiveManager {
    client: Arc<BilibiliLiveClient>,
    session: Option<LiveSession>,
    event_tx: Option<mpsc::Sender<LiveEvent>>,
    broadcaster: Option<broadcast::Sender<String>>,
}

impl LiveManager {
    pub fn new(
        config: BilibiliLiveConfig,
        event_tx: Option<mpsc::Sender<LiveEvent>>,
        broadcaster: Option<broadcast::Sender<String>>,
    ) -> Self {
        let client = Arc::new(BilibiliLiveClient::new(config));
        Self {
            client,
            session: None,
            event_tx,
            broadcaster,
        }
    }

    pub async fn start(&mut self) -> Result<LiveSessionInfo> {
        if self.session.is_some() {
            return Err(AgentError::unsupported("直播长链已连接，先执行 live stop"));
        }

        let code = self
            .client
            .config
            .id_code
            .clone()
            .ok_or_else(|| AgentError::MissingConfig("live.bilibili.id_code"))?;

        let start = self.client.start(&code).await?;
        let ws_url = start
            .websocket_info
            .wss_link
            .first()
            .cloned()
            .ok_or_else(|| AgentError::other("B站返回的 wss_link 为空"))?;

        let session = LiveSession::spawn(
            self.client.clone(),
            ws_url,
            start.websocket_info.auth_body.clone(),
            start.game_info.game_id.clone(),
            start.anchor_info.clone(),
            self.event_tx.clone(),
            self.broadcaster.clone(),
        )
        .await?;

        let info = session.info();
        self.session = Some(session);
        Ok(info)
    }

    pub async fn stop(&mut self) -> Result<Option<LiveSessionInfo>> {
        let session = match self.session.take() {
            Some(session) => session,
            None => return Ok(None),
        };
        let info = session.info();
        session.shutdown(self.client.clone()).await?;
        Ok(Some(info))
    }

    pub fn info(&self) -> Option<LiveSessionInfo> {
        self.session.as_ref().map(LiveSession::info)
    }
}

impl Drop for LiveManager {
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            session.abort();
        }
    }
}

#[derive(Debug, Clone)]
pub struct LiveSessionInfo {
    pub game_id: String,
    pub room_id: i64,
    pub anchor_name: String,
    pub anchor_open_id: Option<String>,
    pub started_at: DateTime<FixedOffset>,
}

#[derive(Debug)]
struct LiveSession {
    info: LiveSessionInfo,
    shutdown_tx: broadcast::Sender<()>,
    task: JoinHandle<Result<()>>,
}

impl LiveSession {
    fn info(&self) -> LiveSessionInfo {
        self.info.clone()
    }

    async fn shutdown(self, client: Arc<BilibiliLiveClient>) -> Result<()> {
        let _ = self.shutdown_tx.send(());
        match self.task.await {
            Ok(result) => {
                if let Err(err) = result {
                    warn!(target: "bilibili::live", error = ?err, "直播长链任务退出时出现错误");
                }
            }
            Err(err) => {
                warn!(target: "bilibili::live", error = ?err, "直播长链任务 JoinHandle 错误");
            }
        }
        client.end(&self.info.game_id).await
    }

    fn abort(self) {
        let _ = self.shutdown_tx.send(());
        self.task.abort();
    }

    async fn spawn(
        client: Arc<BilibiliLiveClient>,
        ws_url: String,
        auth_body: String,
        game_id: String,
        anchor: AnchorInfo,
        event_tx: Option<mpsc::Sender<LiveEvent>>,
        broadcaster: Option<broadcast::Sender<String>>,
    ) -> Result<Self> {
        let (shutdown_tx, _) = broadcast::channel(1);
        let info = LiveSessionInfo {
            game_id: game_id.clone(),
            room_id: anchor.room_id.unwrap_or_default(),
            anchor_name: anchor
                .uname
                .clone()
                .unwrap_or_else(|| "Unknown".to_string()),
            anchor_open_id: anchor.open_id.clone(),
            started_at: now_in_beijing(),
        };

        let shutdown_rx = shutdown_tx.subscribe();
        let task = tokio::spawn(run_live_loop(
            client,
            ws_url,
            auth_body,
            game_id,
            shutdown_rx,
            event_tx,
            broadcaster.clone(),
        ));

        Ok(Self {
            info,
            shutdown_tx,
            task,
        })
    }
}

async fn run_live_loop(
    client: Arc<BilibiliLiveClient>,
    mut ws_url: String,
    auth_body: String,
    game_id: String,
    shutdown_rx: broadcast::Receiver<()>,
    event_tx: Option<mpsc::Sender<LiveEvent>>,
    broadcaster: Option<broadcast::Sender<String>>,
) -> Result<()> {
    if !ws_url.ends_with("/sub") {
        if ws_url.ends_with('/') {
            ws_url.push_str("sub");
        } else {
            ws_url.push_str("/sub");
        }
    }

    info!(target: "bilibili::live", url = %ws_url, "开始连接 B 站直播长链");
    let (ws_stream, _) = connect_async(ws_url)
        .await
        .map_err(|err| AgentError::other(format!("连接 B 站直播长链失败: {err}")))?;
    let (mut writer, mut reader) = ws_stream.split();

    let auth_packet = encode_packet(OP_AUTH, auth_body.as_bytes());
    writer
        .send(Message::Binary(auth_packet.into()))
        .await
        .map_err(|err| AgentError::other(format!("发送鉴权包失败: {err}")))?;

    let mut ws_heartbeat = time::interval(Duration::from_secs(DEFAULT_HEARTBEAT_INTERVAL));
    ws_heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ws_heartbeat.tick().await; // align interval

    let mut api_heartbeat = time::interval(Duration::from_secs(
        client.config.heartbeat_interval_seconds,
    ));
    api_heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    api_heartbeat.tick().await;

    let mut shutdown_stream = BroadcastStream::new(shutdown_rx);

    loop {
        select! {
            _ = ws_heartbeat.tick() => {
                if let Err(err) = writer.send(Message::Binary(encode_packet(OP_HEARTBEAT, &[]).into())).await {
                    warn!(target: "bilibili::live", error = ?err, "发送 WS 心跳失败");
                    break;
                }
            }
            _ = api_heartbeat.tick() => {
                if let Err(err) = client.heartbeat(&game_id).await {
                    warn!(target: "bilibili::live", error = ?err, "调用项目心跳失败");
                }
            }
            maybe_shutdown = shutdown_stream.next() => {
                match maybe_shutdown {
                    Some(Ok(_)) | Some(Err(_)) => {
                        info!(target: "bilibili::live", "收到关闭信号，准备退出直播长链");
                        break;
                    }
                    None => {}
                }
            }
            message = reader.next() => {
                match message {
                    Some(Ok(Message::Binary(payload))) => {
                        handle_packets(&payload, event_tx.as_ref(), broadcaster.as_ref()).await?;
                    }
                    Some(Ok(Message::Text(text))) => {
                        debug!(target: "bilibili::live", %text, "收到文本消息");
                    }
                    Some(Ok(Message::Ping(data))) => {
                        if let Err(err) = writer.send(Message::Pong(data)).await {
                            warn!(target: "bilibili::live", error = ?err, "发送 Pong 失败");
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        info!(target: "bilibili::live", frame = ?frame, "服务器主动关闭连接");
                        break;
                    }
                    Some(Ok(other)) => {
                        debug!(target: "bilibili::live", message = ?other, "收到未处理的 WebSocket 消息");
                    }
                    Some(Err(err)) => {
                        warn!(target: "bilibili::live", error = ?err, "读取直播长链消息失败");
                        break;
                    }
                    None => {
                        info!(target: "bilibili::live", "直播长链已断开");
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

async fn handle_packets(
    payload: &[u8],
    event_tx: Option<&mpsc::Sender<LiveEvent>>,
    broadcaster: Option<&broadcast::Sender<String>>,
) -> Result<()> {
    let packets = decode_packets(payload)?;

    for packet in packets {
        match packet.operation {
            OP_AUTH_REPLY => {
                info!(
                    target: "bilibili::live",
                    packet_len = packet.packet_len,
                    header_len = packet.header_len,
                    version = packet.version,
                    sequence = packet.sequence,
                    "鉴权成功，开始接收直播事件"
                );
            }
            OP_HEARTBEAT_REPLY => {
                debug!(
                    target: "bilibili::live",
                    packet_len = packet.packet_len,
                    header_len = packet.header_len,
                    version = packet.version,
                    sequence = packet.sequence,
                    "收到心跳回包"
                );
            }
            OP_SEND_EVENT => {
                debug!(
                    target: "bilibili::live",
                    packet_len = packet.packet_len,
                    header_len = packet.header_len,
                    version = packet.version,
                    sequence = packet.sequence,
                    "解析直播事件包"
                );
                for event in parse_events(&packet)? {
                    if let Some(broadcaster) = broadcaster {
                        let payload = serde_json::json!({
                            "cmd": event.cmd,
                            "data": event.data,
                        });
                        broadcast_json(broadcaster, "live.event", payload);
                    }
                    if let Some(sender) = event_tx {
                        if let Err(err) = sender.send(event.clone()).await {
                            warn!(target: "bilibili::live", error = ?err, "直播事件投递失败");
                        }
                    }
                    render_event(&event);
                }
            }
            other => {
                debug!(
                    target: "bilibili::live",
                    operation = other,
                    packet_len = packet.packet_len,
                    header_len = packet.header_len,
                    version = packet.version,
                    sequence = packet.sequence,
                    len = packet.body.len(),
                    "收到未处理的消息类型"
                );
            }
        }
    }

    Ok(())
}

fn parse_events(packet: &BiliPacket) -> Result<Vec<LiveEvent>> {
    let mut events = Vec::new();
    let mut slices = packet.body.split(|b| *b == 0);
    while let Some(chunk) = slices.next() {
        if chunk.is_empty() {
            continue;
        }
        match serde_json::from_slice::<LiveMessage>(chunk) {
            Ok(message) => events.push(LiveEvent {
                cmd: message.cmd,
                data: message.data,
            }),
            Err(err) => {
                warn!(target: "bilibili::live", error = ?err, "解析直播 JSON 失败: {}", String::from_utf8_lossy(chunk));
            }
        }
    }
    Ok(events)
}

fn render_event(event: &LiveEvent) {
    match event.cmd.as_str() {
        "LIVE_OPEN_PLATFORM_DM" => {
            let timestamp = format_timestamp(event.field_i64(&["timestamp"]));
            let mut name = event
                .field_str(&["uname"])
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "匿名用户".to_string());
            if event.field_bool(&["is_admin"]).unwrap_or(false) {
                name = format!("🛡️ {}", name);
            }
            let message = event
                .field_str(&["msg"])
                .unwrap_or_else(|| "<空>".to_string());

            let mut details = Vec::new();
            push_detail(&mut details, "open_id", event.field_str(&["open_id"]));
            push_detail(&mut details, "room_id", event.field_i64(&["room_id"]));
            if let Some(level) = event.field_i64(&["guard_level"]).filter(|level| *level > 0) {
                details.push(format!("大航海: {}", guard_level_label(level)));
            }
            if let Some(medal) = format_medal(
                event.field_str(&["fans_medal_name"]),
                event.field_i64(&["fans_medal_level"]),
            ) {
                details.push(medal);
            }
            push_detail(
                &mut details,
                "佩戴粉丝勋章",
                event.field_bool(&["fans_medal_wearing_status"]).map(yes_no),
            );
            if let Some(reply) = event
                .field_str(&["reply_uname"])
                .filter(|reply| !reply.is_empty())
            {
                details.push(format!("回复对象: {}", reply));
            }
            if event.field_i64(&["dm_type"]).unwrap_or(0) == 1 {
                if let Some(url) = event
                    .field_str(&["emoji_img_url"])
                    .filter(|url| !url.is_empty())
                {
                    details.push(format!("表情包: {}", url));
                } else {
                    details.push("表情包弹幕".to_string());
                }
            }
            push_detail(&mut details, "msg_id", event.field_str(&["msg_id"]));

            println!("💬 [{}] {}：{}", timestamp, name, message);
            if !details.is_empty() {
                println!("    {}", details.join(" · "));
            }
        }
        "LIVE_OPEN_PLATFORM_SEND_GIFT" => {
            let timestamp = format_timestamp(event.field_i64(&["timestamp"]));
            let uname = event
                .field_str(&["uname"])
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "匿名用户".to_string());
            let gift = event
                .field_str(&["gift_name"])
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "礼物".to_string());
            let count = event.field_i64(&["gift_num"]).unwrap_or(1).max(1);

            let mut details = Vec::new();
            let price_single = event.field_i64(&["price"]).unwrap_or(0);
            let total = event
                .field_i64(&["r_price"])
                .filter(|value| *value > 0)
                .unwrap_or(price_single * count);
            if total > 0 {
                details.push(format!("价值 {}", format_currency(total)));
            }
            if event.field_bool(&["paid"]).unwrap_or(false) {
                details.push("付费礼物".to_string());
            }
            if event.field_bool(&["combo_gift"]).unwrap_or(false) {
                if let Some(combo_count) = event.field_i64(&["combo_info", "combo_count"]) {
                    details.push(format!("连击 {} 次", combo_count));
                }
                if let Some(combo_base) = event.field_i64(&["combo_info", "combo_base_num"]) {
                    details.push(format!("每次 {} 个", combo_base));
                }
            }
            if let Some(medal) = format_medal(
                event.field_str(&["fans_medal_name"]),
                event.field_i64(&["fans_medal_level"]),
            ) {
                details.push(medal);
            }
            if let Some(level) = event.field_i64(&["guard_level"]).filter(|level| *level > 0) {
                details.push(format!("大航海: {}", guard_level_label(level)));
            }
            push_detail(&mut details, "open_id", event.field_str(&["open_id"]));
            push_detail(&mut details, "room_id", event.field_i64(&["room_id"]));
            push_detail(&mut details, "msg_id", event.field_str(&["msg_id"]));
            push_detail(
                &mut details,
                "礼物图标",
                event
                    .field_str(&["gift_icon"])
                    .filter(|icon| !icon.is_empty()),
            );

            println!("🎁 [{}] {} 送出 {} x{}", timestamp, uname, gift, count);
            if !details.is_empty() {
                println!("    {}", details.join(" · "));
            }
        }
        "LIVE_OPEN_PLATFORM_SUPER_CHAT" => {
            let timestamp = format_timestamp(event.field_i64(&["timestamp"]));
            let uname = event
                .field_str(&["uname"])
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "匿名用户".to_string());
            let amount = event.field_i64(&["rmb"]).unwrap_or(0);
            let message = event
                .field_str(&["message"])
                .unwrap_or_else(|| "<空>".to_string());

            let mut details = Vec::new();
            push_detail(&mut details, "open_id", event.field_str(&["open_id"]));
            push_detail(&mut details, "message_id", event.field_i64(&["message_id"]));
            push_detail(&mut details, "msg_id", event.field_str(&["msg_id"]));
            push_detail(&mut details, "room_id", event.field_i64(&["room_id"]));
            if let Some(medal) = format_medal(
                event.field_str(&["fans_medal_name"]),
                event.field_i64(&["fans_medal_level"]),
            ) {
                details.push(medal);
            }
            if let Some(level) = event.field_i64(&["guard_level"]).filter(|level| *level > 0) {
                details.push(format!("大航海: {}", guard_level_label(level)));
            }
            if let (Some(start), Some(end)) = (
                event.field_i64(&["start_time"]),
                event.field_i64(&["end_time"]),
            ) {
                details.push(format!(
                    "展示时段: {} - {}",
                    format_timestamp(Some(start)),
                    format_timestamp(Some(end))
                ));
            }

            println!(
                "💠 [{}] {} 发送 Super Chat ￥{}：{}",
                timestamp, uname, amount, message
            );
            if !details.is_empty() {
                println!("    {}", details.join(" · "));
            }
        }
        "LIVE_OPEN_PLATFORM_SUPER_CHAT_DEL" => {
            let timestamp = format_timestamp(event.field_i64(&["timestamp"]));
            let ids = event
                .data
                .get("message_ids")
                .and_then(|value| value.as_array())
                .map(|array| {
                    array
                        .iter()
                        .filter_map(|value| value.as_i64())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let id_text = if ids.is_empty() {
                "-".to_string()
            } else {
                ids.iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            };

            println!("🚫 [{}] Super Chat 撤回: {}", timestamp, id_text);
            let mut details = Vec::new();
            push_detail(&mut details, "room_id", event.field_i64(&["room_id"]));
            push_detail(&mut details, "msg_id", event.field_str(&["msg_id"]));
            if !details.is_empty() {
                println!("    {}", details.join(" · "));
            }
        }
        "LIVE_OPEN_PLATFORM_GUARD" => {
            let timestamp = format_timestamp(event.field_i64(&["timestamp"]));
            let uname = event
                .field_str(&["user_info", "uname"])
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "匿名用户".to_string());
            let guard_level = event.field_i64(&["guard_level"]).unwrap_or(0);
            let guard_text = guard_level_label(guard_level);
            let guard_num = event.field_i64(&["guard_num"]).unwrap_or(1);
            let guard_unit = event
                .field_str(&["guard_unit"])
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "月".to_string());

            println!(
                "🛡️ [{}] {} 开通 {} x{} ({})",
                timestamp, uname, guard_text, guard_num, guard_unit
            );

            let mut details = Vec::new();
            let price = event.field_i64(&["price"]).unwrap_or(0);
            if price > 0 {
                details.push(format!("价值 {}", format_currency(price)));
            }
            push_detail(&mut details, "room_id", event.field_i64(&["room_id"]));
            push_detail(
                &mut details,
                "open_id",
                event.field_str(&["user_info", "open_id"]),
            );
            if let Some(medal) = format_medal(
                event.field_str(&["fans_medal_name"]),
                event.field_i64(&["fans_medal_level"]),
            ) {
                details.push(medal);
            }
            push_detail(
                &mut details,
                "佩戴粉丝勋章",
                event.field_bool(&["fans_medal_wearing_status"]).map(yes_no),
            );
            if !details.is_empty() {
                println!("    {}", details.join(" · "));
            }
        }
        "LIVE_OPEN_PLATFORM_LIKE" => {
            let timestamp = format_timestamp(event.field_i64(&["timestamp"]));
            let uname = event
                .field_str(&["uname"])
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "匿名用户".to_string());
            let like_count = event.field_i64(&["like_count"]).unwrap_or(0);

            println!("👍 [{}] {} 点赞 {} 次", timestamp, uname, like_count);

            let mut details = Vec::new();
            if let Some(text) = event
                .field_str(&["like_text"])
                .filter(|text| !text.is_empty())
            {
                details.push(format!("文案: {}", text));
            }
            push_detail(&mut details, "room_id", event.field_i64(&["room_id"]));
            push_detail(&mut details, "open_id", event.field_str(&["open_id"]));
            if !details.is_empty() {
                println!("    {}", details.join(" · "));
            }
        }
        "LIVE_OPEN_PLATFORM_LIVE_ROOM_ENTER" => {
            let timestamp = format_timestamp(event.field_i64(&["timestamp"]));
            let uname = event
                .field_str(&["uname"])
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "匿名用户".to_string());

            println!("🚪 [{}] {} 进入直播间", timestamp, uname);

            let mut details = Vec::new();
            push_detail(&mut details, "room_id", event.field_i64(&["room_id"]));
            push_detail(&mut details, "open_id", event.field_str(&["open_id"]));
            if !details.is_empty() {
                println!("    {}", details.join(" · "));
            }
        }
        "LIVE_OPEN_PLATFORM_LIVE_START" => {
            let timestamp = format_timestamp(event.field_i64(&["timestamp"]));
            let title = event
                .field_str(&["title"])
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "直播开始".to_string());

            println!("🚀 [{}] 直播开始：{}", timestamp, title);

            let mut details = Vec::new();
            push_detail(&mut details, "分区", event.field_str(&["area_name"]));
            push_detail(&mut details, "room_id", event.field_i64(&["room_id"]));
            push_detail(&mut details, "open_id", event.field_str(&["open_id"]));
            if !details.is_empty() {
                println!("    {}", details.join(" · "));
            }
        }
        "LIVE_OPEN_PLATFORM_LIVE_END" => {
            let timestamp = format_timestamp(event.field_i64(&["timestamp"]));
            let title = event
                .field_str(&["title"])
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "直播结束".to_string());

            println!("🏁 [{}] 直播结束：{}", timestamp, title);

            let mut details = Vec::new();
            push_detail(&mut details, "分区", event.field_str(&["area_name"]));
            push_detail(&mut details, "room_id", event.field_i64(&["room_id"]));
            push_detail(&mut details, "open_id", event.field_str(&["open_id"]));
            if !details.is_empty() {
                println!("    {}", details.join(" · "));
            }
        }
        "LIVE_OPEN_PLATFORM_INTERACTION_END" => {
            let timestamp = format_timestamp(event.field_i64(&["timestamp"]));
            let game_id = event
                .field_str(&["game_id"])
                .unwrap_or_else(|| "-".to_string());
            println!("⛔ [{}] 推送结束，game_id: {}", timestamp, game_id);
        }
        other => {
            debug!(target: "bilibili::live", cmd = other, data = ?event.data, "收到直播事件");
        }
    }
}

fn push_detail<T>(details: &mut Vec<String>, label: &str, value: Option<T>)
where
    T: std::fmt::Display,
{
    if let Some(val) = value {
        let text = val.to_string();
        if !text.trim().is_empty() {
            details.push(format!("{}: {}", label, text));
        }
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "是" } else { "否" }
}

fn guard_level_label(level: i64) -> String {
    match level {
        1 => "总督".to_string(),
        2 => "提督".to_string(),
        3 => "舰长".to_string(),
        other => format!("等级 {}", other),
    }
}

fn format_medal(name: Option<String>, level: Option<i64>) -> Option<String> {
    let name = name?;
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }

    match level {
        Some(level) if level > 0 => Some(format!("粉丝勋章: {} Lv{}", trimmed, level)),
        _ => Some(format!("粉丝勋章: {}", trimmed)),
    }
}

fn format_currency(amount: i64) -> String {
    format!("{:.2} 元", amount as f64 / 1000.0)
}

fn format_timestamp(timestamp: Option<i64>) -> String {
    timestamp
        .and_then(timestamp_to_beijing)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "--:--:--".to_string())
}

fn timestamp_to_beijing(timestamp: i64) -> Option<DateTime<FixedOffset>> {
    let offset = FixedOffset::east_opt(8 * 3600)?;
    offset.timestamp_opt(timestamp, 0).single()
}

#[derive(Debug, Clone)]
pub struct LiveEvent {
    pub cmd: String,
    pub data: Value,
}

impl LiveEvent {
    pub fn field_str(&self, path: &[&str]) -> Option<String> {
        let mut current = &self.data;
        for key in path {
            current = current.get(key)?;
        }
        current.as_str().map(|s| s.to_string())
    }

    pub fn field_i64(&self, path: &[&str]) -> Option<i64> {
        let mut current = &self.data;
        for key in path {
            current = current.get(key)?;
        }
        current.as_i64()
    }

    pub fn field_bool(&self, path: &[&str]) -> Option<bool> {
        let mut current = &self.data;
        for key in path {
            current = current.get(key)?;
        }
        current.as_bool()
    }
}

#[derive(Debug, Deserialize)]
struct LiveMessage {
    cmd: String,
    #[serde(default)]
    data: Value,
}

#[derive(Debug, Deserialize, Clone)]
struct StartResponse {
    game_info: GameInfo,
    websocket_info: WebsocketInfo,
    anchor_info: AnchorInfo,
}

#[derive(Debug, Deserialize, Clone)]
struct GameInfo {
    game_id: String,
}

#[derive(Debug, Deserialize, Clone)]
struct WebsocketInfo {
    auth_body: String,
    wss_link: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct AnchorInfo {
    #[serde(default)]
    room_id: Option<i64>,
    #[serde(default)]
    uname: Option<String>,
    #[serde(default)]
    open_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StartApiResponse {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    message: String,
    data: StartResponse,
}

#[derive(Debug, Deserialize)]
struct EmptyApiResponse {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    message: String,
}

#[derive(Debug, Serialize)]
struct StartRequest<'a> {
    code: &'a str,
    app_id: i64,
}

#[derive(Debug, Serialize)]
struct HeartbeatRequest<'a> {
    game_id: &'a str,
}

#[derive(Debug, Serialize)]
struct EndRequest<'a> {
    app_id: i64,
    game_id: &'a str,
}

#[derive(Debug)]
struct BilibiliLiveClient {
    http: reqwest::Client,
    config: BilibiliLiveConfig,
    base_url: String,
}

impl BilibiliLiveClient {
    fn new(config: BilibiliLiveConfig) -> Self {
        let base_url = config
            .host
            .clone()
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("构建 reqwest 客户端失败");
        Self {
            http,
            config,
            base_url,
        }
    }

    async fn start(&self, code: &str) -> Result<StartResponse> {
        let body = serde_json::to_string(&StartRequest {
            code,
            app_id: self.config.app_id,
        })?;
        let response: StartApiResponse = self.post("/v2/app/start", body, Some("start")).await?;

        if response.code != 0 {
            return Err(AgentError::other(format!(
                "B 站 start 接口返回错误: {} {}",
                response.code, response.message
            )));
        }

        Ok(response.data)
    }

    async fn heartbeat(&self, game_id: &str) -> Result<()> {
        let body = serde_json::to_string(&HeartbeatRequest { game_id })?;
        let response: EmptyApiResponse = self
            .post("/v2/app/heartbeat", body, Some("heartbeat"))
            .await?;
        if response.code != 0 {
            warn!(target: "bilibili::live", code = response.code, message = %response.message, "项目心跳失败");
        }
        Ok(())
    }

    async fn end(&self, game_id: &str) -> Result<()> {
        let body = serde_json::to_string(&EndRequest {
            app_id: self.config.app_id,
            game_id,
        })?;
        let response: EmptyApiResponse = self.post("/v2/app/end", body, Some("end")).await?;
        if response.code != 0 {
            warn!(target: "bilibili::live", code = response.code, message = %response.message, "调用 end 接口失败");
        }
        Ok(())
    }

    async fn post<T>(&self, path: &str, body: String, label: Option<&str>) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        let url = format!("{}{}", self.base_url, path);
        let headers = self.build_headers(&body)?;
        let request = self.http.post(url).headers(headers).body(body);

        let label = label.unwrap_or("post");
        let response = request
            .send()
            .await
            .map_err(|err| AgentError::other(format!("调用 B 站 {label} 接口失败: {err}")))?;

        if !response.status().is_success() {
            return Err(AgentError::other(format!(
                "B 站 {label} 接口状态码异常: {}",
                response.status()
            )));
        }

        Ok(response.json::<T>().await?)
    }

    fn build_headers(&self, body: &str) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let mut md5 = Md5::new();
        md5.update(body.as_bytes());
        let content_md5 = format!("{:x}", md5.finalize());

        let timestamp = chrono::Utc::now().timestamp();
        let nonce = Uuid::new_v4().to_string();

        let header_str = format!(
            "x-bili-accesskeyid:{}\nx-bili-content-md5:{}\nx-bili-signature-method:HMAC-SHA256\nx-bili-signature-nonce:{}\nx-bili-signature-version:1.0\nx-bili-timestamp:{}",
            self.config.access_key, content_md5, nonce, timestamp
        );

        let mut mac = Hmac::<Sha256>::new_from_slice(self.config.access_secret.as_bytes())
            .map_err(|err| AgentError::other(format!("创建 HMAC 失败: {err}")))?;
        mac.update(header_str.as_bytes());
        let signature = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{:02x}", byte))
            .collect::<String>();

        headers.insert(
            "x-bili-content-md5",
            HeaderValue::from_str(&content_md5)
                .map_err(|err| AgentError::other(err.to_string()))?,
        );
        headers.insert(
            "x-bili-timestamp",
            HeaderValue::from_str(&timestamp.to_string())
                .map_err(|err| AgentError::other(err.to_string()))?,
        );
        headers.insert(
            "x-bili-signature-method",
            HeaderValue::from_static("HMAC-SHA256"),
        );
        headers.insert(
            "x-bili-signature-nonce",
            HeaderValue::from_str(&nonce).map_err(|err| AgentError::other(err.to_string()))?,
        );
        headers.insert("x-bili-signature-version", HeaderValue::from_static("1.0"));
        headers.insert(
            "x-bili-accesskeyid",
            HeaderValue::from_str(&self.config.access_key)
                .map_err(|err| AgentError::other(err.to_string()))?,
        );
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&signature).map_err(|err| AgentError::other(err.to_string()))?,
        );

        Ok(headers)
    }
}
