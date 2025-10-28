mod capabilities;
mod config;
mod errors;
mod intent;
mod live;
mod orchestrator;
mod providers;
mod sse;
mod util;
// mod ws;  // 已被 SSE 替代，保留文件作为参考

use tracing_subscriber::fmt::{format::Writer, time::FormatTime};

use crate::{
    errors::Result,
    live::LiveSessionInfo,
    orchestrator::AgentController,
    sse::{AgentCommand, BroadcastSender, SignatureAuth},
    util::{format_beijing, now_in_beijing},
};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    init_tracing();

    let config = config::AppConfig::load()?;
    let sse_config = config.sse.clone();
    let bind_addr = sse_config.bind_addr;
    let auth = Arc::new(SignatureAuth::new(
        sse_config.access_key.clone(),
        sse_config.secret_key.clone(),
        sse_config.signature_ttl,
    ));

    let (broadcaster, _bus_rx) = crate::sse::message_bus();
    let (command_tx, mut command_rx) = mpsc::channel(64);

    let mut controller = AgentController::new(config, Some(broadcaster.clone())).await?;

    let sse_task = {
        let broadcaster = broadcaster.clone();
        let auth = auth.clone();
        let command_tx = command_tx.clone();
        tokio::spawn(async move {
            if let Err(err) = crate::sse::run_server(bind_addr, auth, broadcaster, command_tx).await
            {
                error!(target: "sse", error = ?err, "SSE 服务器异常退出");
            }
        })
    };

    broadcast_system_ready(&broadcaster, &controller);

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!(target: "agent", "收到退出信号，准备关闭");
                break;
            }
            Some(event) = controller.recv_live_event(), if controller.has_live_listener() => {
                if let Err(err) = controller.handle_live_event(event).await {
                    error!(target: "agent", error = ?err, "处理直播事件失败");
                    broadcast_error(&broadcaster, "live", &err.to_string());
                }
            }
            command = command_rx.recv() => {
                match command {
                    Some(command) => {
                        handle_agent_command(&mut controller, &broadcaster, command).await?;
                    }
                    None => {
                        error!(target: "agent", "命令通道已关闭，SSE 服务器可能已退出");
                        break;
                    }
                }
            }
        }
    }

    controller.shutdown().await?;

    sse_task.abort();

    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact()
        .with_timer(LocalTimer)
        .init();

    info!("tracing initialized");
}

fn broadcast_system_ready(broadcaster: &BroadcastSender, controller: &AgentController) {
    let capabilities: Vec<_> = controller
        .capabilities_overview()
        .into_iter()
        .map(|(intent, enabled)| {
            json!({
                "intent": intent.to_string(),
                "enabled": enabled,
            })
        })
        .collect();

    crate::sse::broadcast_json(
        broadcaster,
        "system.ready",
        json!({
            "message": "Vutber Agent 已准备就绪",
            "capabilities": capabilities,
            "help": controller.help_message(),
        }),
    );
}

fn broadcast_error(broadcaster: &BroadcastSender, origin: &str, message: &str) {
    crate::sse::broadcast_json(
        broadcaster,
        "agent.error",
        json!({
            "origin": origin,
            "message": message,
        }),
    );
}

async fn handle_agent_command(
    controller: &mut AgentController,
    broadcaster: &BroadcastSender,
    command: AgentCommand,
) -> Result<()> {
    match command {
        AgentCommand::Command { input } => {
            let outcome = controller.handle(&input).await?;
            let (event, mut payload) = outcome.as_event_payload();
            attach_context(&mut payload, "command", Some(json!({ "input": input })));
            crate::sse::broadcast_json(broadcaster, &event, payload);
        }
        AgentCommand::LiveStart => match controller.start_live().await {
            Ok(info) => {
                crate::sse::broadcast_json(broadcaster, "live.started", live_session_payload(&info));
            }
            Err(err) => {
                broadcast_error(broadcaster, "live", &err.to_string());
            }
        },
        AgentCommand::LiveStop => match controller.stop_live().await {
            Ok(Some(info)) => {
                crate::sse::broadcast_json(broadcaster, "live.stopped", live_session_payload(&info));
            }
            Ok(None) => {
                crate::sse::broadcast_json(broadcaster, "live.stopped", json!({ "active": false }));
            }
            Err(err) => {
                broadcast_error(broadcaster, "live", &err.to_string());
            }
        },
        AgentCommand::LiveStatus => match controller.live_status() {
            Ok(Some(info)) => {
                crate::sse::broadcast_json(broadcaster, "live.status", live_session_payload(&info))
            }
            Ok(None) => {
                crate::sse::broadcast_json(broadcaster, "live.status", json!({ "active": false }))
            }
            Err(err) => broadcast_error(broadcaster, "live", &err.to_string()),
        },
    }

    Ok(())
}

fn attach_context(
    payload: &mut serde_json::Value,
    origin: &str,
    context: Option<serde_json::Value>,
) {
    if let serde_json::Value::Object(map) = payload {
        map.insert("origin".to_string(), json!(origin));
        if let Some(context) = context {
            map.insert("context".to_string(), context);
        }
    }
}

fn live_session_payload(info: &LiveSessionInfo) -> serde_json::Value {
    let started_at = format_beijing(&info.started_at, "%Y-%m-%d %H:%M:%S%:z");
    let uptime = now_in_beijing()
        .signed_duration_since(info.started_at)
        .num_seconds()
        .max(0);

    json!({
        "active": true,
        "game_id": info.game_id,
        "room_id": info.room_id,
        "anchor_name": info.anchor_name,
        "anchor_open_id": info.anchor_open_id,
        "started_at": started_at,
        "uptime_seconds": uptime,
    })
}

struct LocalTimer;

impl FormatTime for LocalTimer {
    fn format_time(&self, w: &mut Writer<'_>) -> std::fmt::Result {
        let now = now_in_beijing();
        write!(w, "{}", format_beijing(&now, "%Y-%m-%d %H:%M:%S%:z"))
    }
}
