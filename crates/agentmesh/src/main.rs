//! `AgentMesh` command-line interface and process entry point.

use std::{net::SocketAddr, path::PathBuf};

use agentmesh_config::Config;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tokio::{net::TcpListener, signal};
use tracing::info;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Debug, Parser)]
#[command(name = "agentmesh", version, about = "The service mesh for AI tools")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Starts the `AgentMesh` gateway.
    Serve {
        /// YAML configuration file.
        #[arg(
            short,
            long,
            env = "AGENTMESH_CONFIG",
            default_value = "config/agentmesh.yaml"
        )]
        config: PathBuf,
    },
    /// Validates configuration without starting the gateway.
    Validate {
        /// YAML configuration file.
        #[arg(value_name = "FILE")]
        config: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Serve { config } => serve(config).await,
        Command::Validate { config } => {
            Config::from_path(&config)?;
            println!("configuration is valid: {}", config.display());
            Ok(())
        }
    }
}

async fn serve(path: PathBuf) -> Result<()> {
    let config =
        Config::from_path(&path).with_context(|| format!("failed to load {}", path.display()))?;
    init_telemetry(&config)?;

    let address = SocketAddr::new(config.gateway.host, config.gateway.port);
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("failed to bind gateway to {address}"))?;

    info!(%address, "AgentMesh gateway started");
    axum::serve(listener, agentmesh_gateway::router())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("gateway stopped unexpectedly")
}

fn init_telemetry(config: &Config) -> Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&config.telemetry.filter))?;
    let registry = tracing_subscriber::registry().with(filter);

    if config.telemetry.json {
        registry
            .with(tracing_subscriber::fmt::layer().json())
            .init();
    } else {
        registry.with(tracing_subscriber::fmt::layer()).init();
    }
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("Ctrl+C handler installation");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler installation")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }

    info!("graceful shutdown requested");
}
