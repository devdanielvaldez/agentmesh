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

mod upgrade;

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
    /// Prints a one-shot JSON metrics snapshot from a running gateway.
    Metrics {
        /// Gateway base URL.
        #[arg(
            long,
            env = "AGENTMESH_GATEWAY_URL",
            default_value = "http://127.0.0.1:8080"
        )]
        gateway: String,
    },
    /// Monitors a running gateway live until interrupted.
    Monitor {
        /// Gateway base URL.
        #[arg(
            long,
            env = "AGENTMESH_GATEWAY_URL",
            default_value = "http://127.0.0.1:8080"
        )]
        gateway: String,
        /// Refresh interval in milliseconds.
        #[arg(long, default_value_t = 1000)]
        interval_ms: u64,
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
    /// Checks for a newer release and optionally installs it.
    Upgrade {
        /// Only report the latest release; do not download or install.
        #[arg(long)]
        check: bool,
        /// Install without asking for confirmation.
        #[arg(long)]
        yes: bool,
        /// Install into DIR instead of replacing the running binary.
        #[arg(long, value_name = "DIR")]
        to: Option<PathBuf>,
    },
    /// Evaluates gateway policies for one call without executing it.
    Policy {
        /// YAML configuration file.
        #[arg(short, long, default_value = "config/agentmesh.yaml")]
        config: PathBuf,
        /// Tool name to evaluate (for example `github.get_issue`).
        #[arg(long)]
        tool: String,
        /// MCP method label carrying the tool (default `tools/call`).
        #[arg(long, default_value = "tools/call")]
        method: String,
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

    // Passive update notice: a cache read on most runs, one <=2s network
    // query per day when stale, silent and throttled without network. No
    // check runs for `upgrade` itself; opt out with AGENTMESH_NO_UPDATE_CHECK.
    if std::env::var("AGENTMESH_NO_UPDATE_CHECK").is_err()
        && !matches!(cli.command, Command::Upgrade { .. })
    {
        if let Some(notice) = upgrade::refresh_notice().await {
            eprintln!("{notice}");
        }
    }

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
        Command::Metrics { gateway } => print_metrics(&gateway).await,
        Command::Monitor {
            gateway,
            interval_ms,
        } => monitor_gateway(&gateway, interval_ms).await,
        Command::Schema => {
            println!("{}", serde_json::to_string_pretty(&Config::json_schema())?);
            Ok(())
        }
        Command::Diff { current, candidate } => diff(&current, &candidate),
        Command::Doctor { config } => doctor(&config),
        Command::Upgrade { check, yes, to } => upgrade::run_upgrade(check, yes, to).await,
        Command::Policy {
            config,
            tool,
            method,
        } => policy_check(&config, &tool, &method),
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

async fn fetch_metrics(gateway: &str) -> Result<serde_json::Value> {
    let url = format!("{}/metrics", gateway.trim_end_matches('/'));
    let response = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .with_context(|| format!("failed to reach gateway metrics at {url}"))?;
    if !response.status().is_success() {
        anyhow::bail!("gateway metrics returned {}", response.status());
    }
    response
        .json::<serde_json::Value>()
        .await
        .context("gateway metrics are not valid JSON")
}

async fn print_metrics(gateway: &str) -> Result<()> {
    let metrics = fetch_metrics(gateway).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&metrics).context("failed to format metrics")?
    );
    Ok(())
}

async fn monitor_gateway(gateway: &str, interval_ms: u64) -> Result<()> {
    let interval = Duration::from_millis(interval_ms.max(100));
    loop {
        match fetch_metrics(gateway).await {
            Ok(metrics) => render_dashboard(gateway, &metrics, interval_ms),
            Err(error) => {
                print!("\x1B[2J\x1B[H");
                println!("AgentMesh live — {gateway}\n\nwaiting for gateway: {error:#}");
            }
        }
        let _ = std::io::Write::flush(&mut std::io::stdout());
        tokio::time::sleep(interval).await;
    }
}

/// Renders one dashboard frame from a metrics snapshot.
fn render_dashboard(gateway: &str, metrics: &serde_json::Value, interval_ms: u64) {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "AgentMesh live — {gateway} ({interval_ms}ms refresh, Ctrl-C to quit)\n"
    );
    let _ = writeln!(
        out,
        "uptime {}s | requests {} (errors {})",
        metric_u64(metrics, &["uptime_secs"]),
        metric_u64(metrics, &["requests"]),
        metric_u64(metrics, &["errors"]),
    );
    render_table(&mut out, "METHOD", metrics.get("methods"));
    render_table(&mut out, "UPSTREAM", metrics.get("upstreams"));
    let _ = writeln!(out, "\nRECENT");
    if let Some(recent) = metrics.get("recent").and_then(|value| value.as_array()) {
        for event in recent.iter().rev().take(12) {
            let _ = writeln!(
                out,
                "  {:>4}s {:<22} {:<28} -> {:<8} {} {}ms {}",
                event
                    .get("age_secs")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                event
                    .get("method")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("-"),
                event
                    .get("detail")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("-"),
                event
                    .get("upstream")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("?"),
                event
                    .get("status")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                event
                    .get("latency_ms")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                event
                    .get("policy")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("-"),
            );
        }
    }
    print!("\x1B[2J\x1B[H{out}");
    let _ = std::io::Write::flush(&mut std::io::stdout());
}

/// Renders one aggregated counters table.
fn render_table(out: &mut String, title: &str, table: Option<&serde_json::Value>) {
    use std::fmt::Write as _;
    let _ = writeln!(
        out,
        "\n{title:<22} {:>6} {:>6} {:>7} {:>7}",
        "REQ", "ERR", "AVGms", "MAXms"
    );
    let mut rows: Vec<(&str, &serde_json::Value)> = table
        .and_then(|value| value.as_object())
        .map(|map| map.iter().map(|(name, row)| (name.as_str(), row)).collect())
        .unwrap_or_default();
    rows.sort_by_key(|(_, row)| {
        std::cmp::Reverse(
            row.get("requests")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
        )
    });
    for (name, row) in rows {
        let _ = writeln!(
            out,
            "{name:<22} {:>6} {:>6} {:>7} {:>7}",
            row.get("requests")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            row.get("errors")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            row.get("avg_latency_ms")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            row.get("max_latency_ms")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
        );
    }
}

/// Reads one unsigned metric with a safe default.
fn metric_u64(metrics: &serde_json::Value, path: &[&str]) -> u64 {
    let mut current = metrics;
    for key in path {
        current = match current.get(key) {
            Some(next) => next,
            None => return 0,
        };
    }
    current.as_u64().unwrap_or(0)
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
        "ok upstreams configured: {}",
        config.gateway.effective_upstreams().len()
    );
    if config.gateway.policies.is_empty() {
        println!("ok policies: none configured");
    } else {
        let engine = agentmesh_gateway::PolicyEngine::new(&config.gateway)
            .with_context(|| format!("invalid gateway policies in {}", path.display()))?;
        println!(
            "ok policies: {} rule(s), environment {}",
            config.gateway.policies.len(),
            engine.environment()
        );
    }
    Ok(())
}

/// Dry-runs the policy engine for one tool call: prints the decision without
/// charging budgets or touching any upstream. Exits 2 when denied.
fn policy_check(path: &PathBuf, tool: &str, method: &str) -> Result<()> {
    let config =
        Config::from_path(path).with_context(|| format!("failed to load {}", path.display()))?;
    if config.gateway.policies.is_empty() {
        println!("Decision: ALLOW (no policies configured)");
        return Ok(());
    }
    let engine = agentmesh_gateway::PolicyEngine::new(&config.gateway)
        .with_context(|| format!("invalid gateway policies in {}", path.display()))?;
    let tool_name = if method == "tools/call" {
        Some(tool)
    } else {
        None
    };
    let evaluation = engine.evaluate(method, tool_name);
    match &evaluation.decision {
        agentmesh_gateway::PolicyDecision::Allow {
            rule,
            route,
            redact,
        } => {
            println!("Decision: ALLOW");
            match rule {
                Some(index) => println!("Matched rule: #{index}"),
                None => println!("Matched rule: none (default allow)"),
            }
            if let Some(target) = route.and_then(|index| engine.upstream_name(index)) {
                println!("Route: {target}");
            }
            if let Some(slot) = evaluation.budget_slot {
                if let Some((limit, remaining)) = engine.budget_remaining(slot) {
                    println!("Budget: {remaining}/{limit} calls remaining in the rolling hour");
                }
            }
            if !redact.is_empty() {
                println!("Redactions: {}", redact.join(", "));
            }
            Ok(())
        }
        agentmesh_gateway::PolicyDecision::Deny {
            rule,
            reason,
            quota,
        } => {
            println!("Decision: DENY");
            println!("Matched rule: #{rule}");
            println!("Reason: {reason}");
            if *quota {
                println!("Hint: the hourly budget is exhausted; retry later or raise it.");
            }
            std::process::exit(2);
        }
    }
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

    let policy = if config.gateway.policies.is_empty() {
        None
    } else {
        let engine = agentmesh_gateway::PolicyEngine::new(&config.gateway)
            .with_context(|| format!("invalid gateway policies in {}", path.display()))?;
        info!(
            rules = config.gateway.policies.len(),
            environment = %engine.environment(),
            "Policy routing engine initialized"
        );
        Some(Arc::new(engine))
    };
    let upstreams = config.gateway.effective_upstreams();
    let app = match upstreams.len() {
        0 => agentmesh_gateway::router(),
        1 => {
            let upstream = upstreams[0];
            let endpoint = UpstreamEndpoint::parse(&upstream.url, upstream.allow_insecure_http)
                .context("invalid gateway upstream")?;
            let proxy = ProxyClient::new(ProxyConfig {
                request_timeout: Duration::from_millis(upstream.request_timeout_ms),
                ..ProxyConfig::default()
            })
            .context("failed to initialize MCP proxy")?;
            match &policy {
                Some(engine) => agentmesh_gateway::router_with_upstream_and_policy(
                    proxy,
                    endpoint,
                    Arc::clone(engine),
                ),
                None => agentmesh_gateway::router_with_upstream(proxy, endpoint),
            }
        }
        count => {
            let proxy = ProxyClient::new(ProxyConfig::default())
                .context("failed to initialize MCP proxy")?;
            let mut targets = Vec::with_capacity(count);
            for upstream in upstreams {
                let endpoint = UpstreamEndpoint::parse(&upstream.url, upstream.allow_insecure_http)
                    .context("invalid gateway upstream")?;
                targets.push(agentmesh_proxy::UpstreamTarget {
                    endpoint,
                    timeout: Duration::from_millis(upstream.request_timeout_ms),
                });
            }
            let fanout = agentmesh_proxy::MultiUpstreamProxy::new(proxy, targets)
                .context("invalid multi-upstream configuration")?;
            fanout
                .initialize()
                .await
                .context("failed to discover multi-upstream capabilities")?;
            info!(count, "Multi-upstream fan-out initialized");
            match &policy {
                Some(engine) => agentmesh_gateway::router_with_upstreams_and_policy(
                    Arc::new(fanout),
                    Arc::clone(engine),
                ),
                None => agentmesh_gateway::router_with_upstreams(Arc::new(fanout)),
            }
        }
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
