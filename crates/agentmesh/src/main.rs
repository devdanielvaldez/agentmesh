//! `AgentMesh` command-line interface and process entry point.

use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use agentmesh_config::Config;
use agentmesh_control_api::AdminToken;
use agentmesh_control_plane::ControlPlane;
use agentmesh_proxy::{ProxyClient, ProxyConfig, UpstreamEndpoint};
use agentmesh_storage::SqliteStore;
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
    /// Starts the administrative control plane with durable local storage.
    ControlPlane {
        /// Interface to bind.
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        /// Administrative API port.
        #[arg(long, default_value_t = 8081)]
        port: u16,
        /// `SQLite` database path.
        #[arg(long, default_value = "agentmesh-control.db")]
        database: PathBuf,
        /// Administrative bearer token.
        #[arg(long, env = "AGENTMESH_ADMIN_TOKEN", hide_env_values = true)]
        token: String,
    },
    /// Validates configuration without starting the gateway.
    Validate {
        /// YAML configuration file.
        #[arg(value_name = "FILE")]
        config: PathBuf,
    },
    /// Prints the configuration JSON Schema.
    Schema,
    /// Compares two validated configuration files.
    Diff {
        /// Currently active configuration.
        current: PathBuf,
        /// Candidate configuration.
        candidate: PathBuf,
    },
    /// Checks local configuration and runtime prerequisites.
    Doctor {
        /// YAML configuration file.
        #[arg(short, long, default_value = "config/agentmesh.yaml")]
        config: PathBuf,
    },
    /// Applies one desired-state resource through the control API.
    Apply {
        /// Control API base URL.
        #[arg(long, env = "AGENTMESH_CONTROL_URL")]
        control_url: String,
        /// Organization/tenant.
        #[arg(long)]
        tenant: String,
        /// Namespace.
        #[arg(long)]
        namespace: String,
        /// JSON desired-resource file.
        file: PathBuf,
        /// Administrative bearer token.
        #[arg(long, env = "AGENTMESH_ADMIN_TOKEN", hide_env_values = true)]
        token: String,
        /// Expected revision for an update.
        #[arg(long)]
        revision: Option<u64>,
    },
    /// Reconciles desired state into a gateway snapshot.
    Reconcile {
        /// Control API base URL.
        #[arg(long, env = "AGENTMESH_CONTROL_URL")]
        control_url: String,
        /// Organization/tenant.
        #[arg(long)]
        tenant: String,
        /// Namespace.
        #[arg(long)]
        namespace: String,
        /// Administrative bearer token.
        #[arg(long, env = "AGENTMESH_ADMIN_TOKEN", hide_env_values = true)]
        token: String,
    },
    /// Retrieves the active gateway snapshot.
    Snapshot {
        /// Control API base URL.
        #[arg(long, env = "AGENTMESH_CONTROL_URL")]
        control_url: String,
        /// Organization/tenant.
        #[arg(long)]
        tenant: String,
        /// Namespace.
        #[arg(long)]
        namespace: String,
        /// Administrative bearer token.
        #[arg(long, env = "AGENTMESH_ADMIN_TOKEN", hide_env_values = true)]
        token: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Serve { config } => serve(config).await,
        Command::ControlPlane {
            host,
            port,
            database,
            token,
        } => serve_control_plane(host, port, database, &token).await,
        Command::Validate { config } => {
            Config::from_path(&config)?;
            println!("configuration is valid: {}", config.display());
            Ok(())
        }
        Command::Schema => {
            println!("{}", serde_json::to_string_pretty(&Config::json_schema())?);
            Ok(())
        }
        Command::Diff { current, candidate } => diff(&current, &candidate),
        Command::Doctor { config } => doctor(&config),
        Command::Apply {
            control_url,
            tenant,
            namespace,
            file,
            token,
            revision,
        } => apply_resource(&control_url, &tenant, &namespace, file, &token, revision).await,
        Command::Reconcile {
            control_url,
            tenant,
            namespace,
            token,
        } => {
            control_post(
                &control_url,
                &format!("/v1/scopes/{tenant}/{namespace}/reconcile"),
                &token,
                None,
            )
            .await
        }
        Command::Snapshot {
            control_url,
            tenant,
            namespace,
            token,
        } => {
            control_get(
                &control_url,
                &format!("/v1/scopes/{tenant}/{namespace}/snapshot"),
                &token,
            )
            .await
        }
    }
}

async fn serve_control_plane(
    host: IpAddr,
    port: u16,
    database: PathBuf,
    token: &str,
) -> Result<()> {
    if port == 0 {
        anyhow::bail!("control-plane port must be greater than zero");
    }
    let store = SqliteStore::open(&database).context("failed to open control-plane database")?;
    let plane = Arc::new(ControlPlane::new(store));
    let app = agentmesh_control_api::router(plane, AdminToken::new(token)?);
    let address = SocketAddr::new(host, port);
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("failed to bind control plane to {address}"))?;
    info!(%address, database = %database.display(), "AgentMesh control plane started");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("control plane stopped unexpectedly")
}

fn diff(current: &PathBuf, candidate: &PathBuf) -> Result<()> {
    let current_config = Config::from_path(current)?;
    let candidate_config = Config::from_path(candidate)?;
    let current_yaml = serde_yaml::to_string(&current_config)?;
    let candidate_yaml = serde_yaml::to_string(&candidate_config)?;
    if current_yaml == candidate_yaml {
        println!("no configuration changes");
    } else {
        println!(
            "configuration differs\n--- current\n{current_yaml}--- candidate\n{candidate_yaml}"
        );
    }
    Ok(())
}

fn doctor(path: &PathBuf) -> Result<()> {
    let config = Config::from_path(path)
        .with_context(|| format!("failed to validate {}", path.display()))?;
    let address = SocketAddr::new(config.gateway.host, config.gateway.port);
    println!("ok configuration: {}", path.display());
    println!("ok gateway address: {address}");
    println!(
        "ok upstream configured: {}",
        config.gateway.upstream.is_some()
    );
    Ok(())
}

async fn apply_resource(
    control_url: &str,
    tenant: &str,
    namespace: &str,
    file: PathBuf,
    token: &str,
    revision: Option<u64>,
) -> Result<()> {
    let bytes =
        std::fs::read(&file).with_context(|| format!("failed to read {}", file.display()))?;
    let resource: serde_json::Value =
        serde_json::from_slice(&bytes).context("resource file is not valid JSON")?;
    let body = serde_json::json!({"resource": resource, "expected_revision": revision});
    control_post(
        control_url,
        &format!("/v1/scopes/{tenant}/{namespace}/resources"),
        token,
        Some(body),
    )
    .await
}

async fn control_post(
    base: &str,
    path: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> Result<()> {
    let client = reqwest::Client::new();
    let mut request = client
        .post(format!("{}{path}", base.trim_end_matches('/')))
        .bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    print_control_response(request.send().await?).await
}

async fn control_get(base: &str, path: &str, token: &str) -> Result<()> {
    let response = reqwest::Client::new()
        .get(format!("{}{path}", base.trim_end_matches('/')))
        .bearer_auth(token)
        .send()
        .await?;
    print_control_response(response).await
}

async fn print_control_response(response: reqwest::Response) -> Result<()> {
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("control API returned {status}: {body}");
    }
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        println!("{body}");
    }
    Ok(())
}

async fn serve(path: PathBuf) -> Result<()> {
    let config =
        Config::from_path(&path).with_context(|| format!("failed to load {}", path.display()))?;
    init_telemetry(&config)?;

    let address = SocketAddr::new(config.gateway.host, config.gateway.port);
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("failed to bind gateway to {address}"))?;

    let app = if let Some(upstream) = &config.gateway.upstream {
        let endpoint = UpstreamEndpoint::parse(&upstream.url, upstream.allow_insecure_http)
            .context("invalid gateway upstream")?;
        let proxy = ProxyClient::new(ProxyConfig {
            request_timeout: Duration::from_millis(upstream.request_timeout_ms),
            ..ProxyConfig::default()
        })
        .context("failed to initialize MCP proxy")?;
        agentmesh_gateway::router_with_upstream(proxy, endpoint)
    } else {
        agentmesh_gateway::router()
    };

    info!(%address, "AgentMesh gateway started");
    axum::serve(listener, app)
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
