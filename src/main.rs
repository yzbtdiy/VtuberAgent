use anyhow::Result;
use std::env;
use tracing::{Level, info};
use tracing_subscriber;

mod agents;
mod api;
mod auth;
mod config;
mod memory;
mod models;
mod services;
mod tools;
mod workflows;

use api::websocket_server::WebSocketServer;
use config::Settings;

#[tokio::main]
async fn main() -> Result<()> {
    // Load configuration first
    let settings = Settings::load()?;

    // Set environment variables based on configuration
    unsafe {
        env::set_var("RUST_LOG", &settings.logging.rust_log);
        if settings.logging.otel_sdk_disabled {
            env::set_var("OTEL_SDK_DISABLED", "true");
        }
        if settings.logging.crewai_telemetry_disabled {
            env::set_var("CREWAI_TELEMETRY_DISABLED", "true");
        }
    }

    // Initialize tracing after setting environment variables
    tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_target(false)
        .init();

    info!("Starting VTuber Rig Server");
    info!("Configuration loaded successfully");

    // Start WebSocket server
    let server = WebSocketServer::new(settings).await?;
    server.start().await?;

    Ok(())
}
