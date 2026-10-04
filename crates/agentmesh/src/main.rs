//! `AgentMesh` command-line interface and process entry point.

use std::{
    io::{IsTerminal, Write},
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

mod teach_flow;
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
    /// Authors a workflow: guided manual authoring, or browser recording.
    Teach {
        /// Workflow id (for example `whatsapp.read_messages`).
        #[arg(long)]
        name: Option<String>,
        /// Authoring mode: `manual` (guided prompts) or `browser` (recorded).
        #[arg(long, default_value = "manual")]
        target: String,
        /// Recording scope origin for browser mode.
        #[arg(long)]
        scope: Option<String>,
        /// URL opened for a browser demonstration.
        #[arg(long)]
        start_url: Option<String>,
        /// Show the recording browser window.
        #[arg(long)]
        headed: bool,
        /// Record or run with a stored session profile.
        #[arg(long)]
        session: Option<String>,
        /// Learn a second demonstration into an existing workflow.
        #[arg(long = "continue")]
        continue_from: Option<String>,
        /// Expose the recording browser on a CDP port (advanced automation).
        #[arg(long)]
        cdp_port: Option<u16>,
        /// Keep every recorded event as its own literal step (no dedupe, no candidates).
        #[arg(long)]
        raw: bool,
    },
    /// Manages stored workflows.
    Workflows {
        #[command(subcommand)]
        action: WorkflowsAction,
    },
    /// Manages persistent application sessions.
    Sessions {
        #[command(subcommand)]
        action: SessionsAction,
    },
    /// Inspects secret references used by stored workflows.
    Secret {
        #[command(subcommand)]
        action: SecretAction,
    },
    /// Re-executes a recorded run with its original inputs.
    Replay {
        /// Executor run id from a previous run.
        run_id: String,
        /// Skip write confirmations.
        #[arg(long)]
        yes: bool,
        /// Override stored inputs as key=value pairs (repeatable).
        #[arg(long = "input", value_name = "KEY=VALUE")]
        input: Vec<String>,
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

/// Stored-workflow operations.
#[derive(Debug, Subcommand)]
enum WorkflowsAction {
    /// Lists stored workflows.
    List {
        /// Report corrupt documents instead of failing on the first one.
        #[arg(long)]
        lenient: bool,
    },
    /// Prints one stored workflow as YAML.
    Inspect {
        /// Workflow id.
        id: String,
    },
    /// Validates a workflow file or a stored workflow id.
    Validate {
        /// File path or stored workflow id.
        target: String,
    },
    /// Deletes a stored workflow (keeps a revision snapshot).
    Delete {
        /// Workflow id.
        id: String,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Renames a stored workflow, rewriting its id.
    Rename {
        /// Current workflow id.
        from: String,
        /// New workflow id (`namespace.name`).
        to: String,
    },
    /// Prints a step-level diff between two stored workflows.
    Diff {
        /// First workflow id.
        a: String,
        /// Second workflow id.
        b: String,
    },
    /// Imports a workflow YAML file into the store.
    Import {
        /// Workflow file to import.
        file: PathBuf,
        /// Overwrite the stored workflow when the id already exists.
        #[arg(long)]
        overwrite: bool,
    },
    /// Applies repair candidates proposed by self-healing runs.
    ApplyRepair {
        /// Workflow id.
        id: String,
        /// Only this step id (default: every candidate for the workflow).
        #[arg(long)]
        step: Option<String>,
        /// Skip per-repair confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Generalizes brittle recorded assertions (volatile URLs, duplicates).
    Relax {
        /// Workflow id.
        id: String,
    },
    /// Removes old run directories and stale repair candidates.
    Prune {
        /// Remove runs older than this many days.
        #[arg(long, default_value_t = 30)]
        older_than_days: u64,
        /// Always keep this many of the newest run directories.
        #[arg(long, default_value_t = 10)]
        keep_last: usize,
        /// List what would be removed without removing it.
        #[arg(long)]
        dry_run: bool,
    },
    /// Runs a JSON cases file against a workflow and checks expected outputs.
    Eval {
        /// Workflow id.
        id: String,
        /// JSON cases file ([{inputs, expect}]).
        #[arg(long)]
        cases: PathBuf,
        /// Skip write confirmations.
        #[arg(long)]
        yes: bool,
        /// Show the runtime browser window while executing.
        #[arg(long)]
        headed: bool,
    },
    /// Shows per-step reliability from run audits (retries, heals, repairs).
    Flaky {
        /// Workflow id. Omit for every taught workflow.
        id: Option<String>,
    },
    /// Promotes the most repeated routine into a saved sub-workflow.
    Compose {
        /// Namespace for the composed workflow id.
        #[arg(long)]
        namespace: String,
        /// Name for the composed workflow id.
        #[arg(long)]
        name: String,
        /// Minimum routine length in steps.
        #[arg(long, default_value_t = 2)]
        min_steps: usize,
        /// Maximum routine length in steps.
        #[arg(long, default_value_t = 4)]
        max_steps: usize,
    },
    /// Generates a reviewed starter API adapter from observed APIs.
    GenApiAdapter {
        /// Workflow id.
        id: String,
        /// Output executable path.
        #[arg(long)]
        out: PathBuf,
    },
    /// Checks that a runtime adapter honors the Teach contract.
    CheckAdapter {
        /// Runtime the adapter serves (`desktop`, `mobile`, `api`).
        #[arg(long)]
        runtime: String,
        /// Adapter executable path.
        #[arg(long)]
        adapter: String,
    },
    /// Executes a stored workflow through the Playwright runtime.
    Run {
        /// Workflow id.
        id: String,
        /// Inputs as key=value pairs (repeatable).
        #[arg(long = "input", value_name = "KEY=VALUE")]
        input: Vec<String>,
        /// Resolve targets and stop before the first write.
        #[arg(long)]
        dry_run: bool,
        /// Skip write confirmations.
        #[arg(long)]
        yes: bool,
        /// Run inside a stored session profile.
        #[arg(long)]
        session: Option<String>,
        /// Show the runtime browser window while executing.
        #[arg(long)]
        headed: bool,
    },
    /// Validates a workflow and dry-runs it without performing writes.
    Test {
        /// Workflow id.
        id: String,
        /// Inputs as key=value pairs (repeatable).
        #[arg(long = "input", value_name = "KEY=VALUE")]
        input: Vec<String>,
    },
    /// Exports a workflow to another format.
    Export {
        /// Workflow id. Omit it together with --all to export the capability library.
        id: Option<String>,
        /// Export every stored workflow as one MCP capability library.
        #[arg(long, conflicts_with = "id")]
        all: bool,
        /// Export format (`mcp`).
        #[arg(long)]
        target: String,
        /// Output directory. Defaults to the platform `AgentMesh` data directory under `mcp/`.
        #[arg(long)]
        out: Option<String>,
    },
    /// Writes a ready-to-use MCP client config for an exported server.
    ClientConfig {
        /// Client flavor (`claude-code`, `claude-desktop`, `generic`).
        #[arg(long)]
        client: String,
        /// Exported `server.mjs` path.
        #[arg(long)]
        server: String,
        /// Session profile: application name (`linkedin`) or profile directory.
        #[arg(long)]
        profile: Option<String>,
        /// Output file.
        #[arg(long)]
        out: String,
    },
    /// Exports redacted successful/failed trajectories for evaluation or model training.
    Dataset {
        /// Destination JSONL file.
        #[arg(long)]
        out: PathBuf,
        /// Only this workflow id.
        #[arg(long)]
        workflow: Option<String>,
        /// Only this status (`succeeded` or `failed`).
        #[arg(long)]
        status: Option<String>,
        /// Only records from the last N days.
        #[arg(long)]
        since_days: Option<u64>,
        /// Maximum records to export (0 means no limit).
        #[arg(long, default_value_t = 0)]
        limit: usize,
    },
    /// Summarizes observed execution quality from local experience memory.
    Report {
        /// Workflow id. Omit to aggregate every taught workflow.
        id: Option<String>,
        /// Only records from the last N days.
        #[arg(long)]
        since_days: Option<u64>,
        /// Only this status (`succeeded` or `failed`).
        #[arg(long)]
        status: Option<String>,
    },
}

/// Application session operations.
#[derive(Debug, Subcommand)]
enum SessionsAction {
    /// Opens an application once so the user can log in; the session persists.
    Login {
        /// Application name (`whatsapp`).
        app: String,
        /// Login URL.
        #[arg(long)]
        url: String,
    },
    /// Lists stored application sessions.
    List,
}

/// Secret inventory operations.
#[derive(Debug, Subcommand)]
enum SecretAction {
    /// Lists secret references used by stored workflows and their status.
    List,
    /// Stores a secret value in `$AGENTMESH_HOME/.env` (mode 0600).
    Set {
        /// Secret variable (must start with SECRET_).
        variable: String,
        /// Value (prefer the prompt; flags stay in shell history).
        #[arg(long)]
        value: Option<String>,
    },
    /// Replaces the stored value for a secret variable.
    Rotate {
        /// Secret variable (must start with SECRET_).
        variable: String,
        /// Value (prefer the prompt; flags stay in shell history).
        #[arg(long)]
        value: Option<String>,
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
        teach @ (Command::Teach { .. }
        | Command::Workflows { .. }
        | Command::Sessions { .. }
        | Command::Secret { .. }
        | Command::Replay { .. }) => dispatch_teach(teach),
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

/// Runs the Teach family of subcommands: authoring, workflows, sessions,
/// secrets, and replay. Anything else is a programming error by the caller.
#[allow(clippy::too_many_arguments)]
fn dispatch_teach_mode(
    name: Option<String>,
    target: &str,
    scope: Option<String>,
    start_url: Option<String>,
    headed: bool,
    session: Option<String>,
    continue_from: Option<String>,
    cdp_port: Option<u16>,
    raw: bool,
) -> Result<()> {
    match target {
        "browser" => teach_flow::teach_browser(&teach_flow::BrowserTeachOptions {
            name,
            scope,
            start_url,
            headed,
            session,
            continue_from,
            cdp_port,
            raw,
        }),
        "manual" => {
            if scope.is_some()
                || start_url.is_some()
                || headed
                || session.is_some()
                || continue_from.is_some()
                || cdp_port.is_some()
                || raw
            {
                anyhow::bail!("browser-only flags need --target browser");
            }
            teach_wizard(name)
        }
        other => anyhow::bail!("unknown teach target {other:?}; use manual or browser"),
    }
}

/// Stored-workflow catalog operations (list through repair).
fn dispatch_workflows_catalog(action: &WorkflowsAction) -> Option<Result<()>> {
    match action {
        WorkflowsAction::List { lenient } => Some(workflows_list(*lenient)),
        WorkflowsAction::Inspect { id } => Some(workflows_inspect(id)),
        WorkflowsAction::Validate { target } => Some(workflows_validate(target)),
        WorkflowsAction::Delete { id, yes } => Some(teach_flow::workflows_delete(id, *yes)),
        WorkflowsAction::Rename { from, to } => Some(teach_flow::workflows_rename(from, to)),
        WorkflowsAction::Diff { a, b } => Some(teach_flow::workflows_diff(a, b)),
        WorkflowsAction::Import { file, overwrite } => Some(teach_flow::workflows_import(
            &file.to_string_lossy(),
            *overwrite,
        )),
        WorkflowsAction::ApplyRepair { id, step, yes } => Some(teach_flow::workflows_apply_repair(
            id,
            step.as_deref(),
            *yes,
        )),
        WorkflowsAction::Relax { id } => Some(teach_flow::workflows_relax(id)),
        WorkflowsAction::ClientConfig {
            client,
            server,
            profile,
            out,
        } => Some(teach_flow::workflows_client_config(
            client,
            server,
            profile.as_deref(),
            out,
        )),
        _ => None,
    }
}

/// Stored-workflow lifecycle operations (prune through report).
fn dispatch_workflows_lifecycle(action: &WorkflowsAction) -> Result<()> {
    match action {
        WorkflowsAction::Prune {
            older_than_days,
            keep_last,
            dry_run,
        } => teach_flow::workflows_prune(*older_than_days, *keep_last, *dry_run),
        WorkflowsAction::Eval {
            id,
            cases,
            yes,
            headed,
        } => teach_flow::workflows_eval(id, &cases.to_string_lossy(), *yes, *headed),
        WorkflowsAction::Flaky { id } => teach_flow::workflows_flaky(id.as_deref()),
        WorkflowsAction::Compose {
            namespace,
            name,
            min_steps,
            max_steps,
        } => teach_flow::workflows_compose(namespace, name, *min_steps, *max_steps),
        WorkflowsAction::GenApiAdapter { id, out } => {
            teach_flow::workflows_gen_api_adapter(id, &out.to_string_lossy())
        }
        WorkflowsAction::CheckAdapter { runtime, adapter } => {
            teach_flow::workflows_check_adapter(runtime, adapter)
        }
        other => dispatch_workflows_execute(other),
    }
}

fn dispatch_teach(command: Command) -> Result<()> {
    match command {
        Command::Teach {
            name,
            target,
            scope,
            start_url,
            headed,
            session,
            continue_from,
            cdp_port,
            raw,
        } => dispatch_teach_mode(
            name,
            &target,
            scope,
            start_url,
            headed,
            session,
            continue_from,
            cdp_port,
            raw,
        ),
        Command::Workflows { action } => {
            if let Some(done) = dispatch_workflows_catalog(&action) {
                return done;
            }
            dispatch_workflows_lifecycle(&action)
        }
        rest => dispatch_teach_rest(rest),
    }
}
/// Stored-workflow execution operations (run through report).
fn dispatch_workflows_execute(action: &WorkflowsAction) -> Result<()> {
    match action {
        WorkflowsAction::Run {
            id,
            input,
            dry_run,
            yes,
            session,
            headed,
        } => teach_flow::workflows_run(
            id,
            &teach_flow::RunOptions {
                inputs: input.clone(),
                dry_run: *dry_run,
                yes: *yes,
                session: session.clone(),
                headed: *headed,
            },
        ),
        WorkflowsAction::Test { id, input } => teach_flow::workflows_test(id, input),
        WorkflowsAction::Export {
            id,
            all,
            target,
            out,
        } => {
            if !all && id.is_none() {
                anyhow::bail!("provide a workflow id or use --all");
            }
            teach_flow::workflows_export(id.as_deref(), *all, target, out.as_deref())
        }
        WorkflowsAction::Dataset {
            out,
            workflow,
            status,
            since_days,
            limit,
        } => teach_flow::workflows_dataset(
            out,
            &teach_flow::ExperienceFilter {
                workflow: workflow.clone(),
                status: status.clone(),
                since_days: *since_days,
            },
            *limit,
        ),
        WorkflowsAction::Report {
            id,
            since_days,
            status,
        } => teach_flow::workflows_report(&teach_flow::ExperienceFilter {
            workflow: id.clone(),
            status: status.clone(),
            since_days: *since_days,
        }),
        other => anyhow::bail!("not a workflow execution command: {other:?}"),
    }
}

/// Sessions, secrets, and replay operations.
fn dispatch_teach_rest(command: Command) -> Result<()> {
    match command {
        Command::Sessions { action } => match action {
            SessionsAction::Login { app, url } => teach_flow::sessions_login(&app, &url),
            SessionsAction::List => teach_flow::sessions_list(),
        },
        Command::Secret { action } => match action {
            SecretAction::List => teach_flow::secret_list(),
            SecretAction::Set { variable, value } | SecretAction::Rotate { variable, value } => {
                teach_flow::secret_set(&variable, value.as_deref())
            }
        },
        Command::Replay { run_id, yes, input } => teach_flow::replay_run(&run_id, yes, &input),
        _ => anyhow::bail!("not a teach command"),
    }
}

/// Guided workflow authoring: prompts for description, inputs, steps, and
/// outputs, validates the IR, and saves it to the local store.
fn teach_wizard(name: Option<String>) -> Result<()> {
    use agentmesh_teach::{InputDef, Workflow};
    if !std::io::stdin().is_terminal() {
        anyhow::bail!("teach is interactive; run it in a terminal");
    }
    println!("Teach AgentMesh a reusable capability.");
    println!("Values may reference {{{{ inputs.<name> }}}} and {{{{ steps.<id>[.result] }}}}.");
    let id = match name {
        Some(name) if !name.trim().is_empty() => name,
        _ => teach_prompt("Workflow id (app.capability)")?,
    };
    let description = teach_prompt("Description (optional)")?;
    let runtime = teach_prompt_default("Preferred runtime", "browser")?;
    let mut inputs = std::collections::BTreeMap::new();
    loop {
        let name = teach_prompt("Input name (empty to finish)")?;
        if name.is_empty() {
            break;
        }
        let kind = teach_prompt_default("Type [string]", "string")?;
        let input_type = parse_input_type(&kind)?;
        let default = teach_default_value(&teach_prompt("Default (empty for none)")?, input_type)?;
        let required = if default.is_some() {
            Some(false)
        } else {
            Some(teach_yes_no("Required?", true)?)
        };
        inputs.insert(
            name.clone(),
            InputDef {
                input_type,
                required,
                default,
            },
        );
        println!("  added input {name}");
    }
    println!("Known ops: browser.* ui.* app.* file.* auth.* control.* assert.* human.*");
    println!("(see docs/WORKFLOW_IR.md for the full list)");
    let steps = teach_wizard_steps()?;
    if steps.is_empty() {
        anyhow::bail!("a workflow needs at least one step");
    }
    let outputs = teach_wizard_outputs()?;
    let policy = teach_policy()?;
    let checkpoints = steps
        .iter()
        .find(|step| step.op == "browser.navigate")
        .map(|step| vec![step.id.clone()])
        .unwrap_or_default();
    let workflow = Workflow {
        version: agentmesh_teach::SUPPORTED_IR_VERSION.to_string(),
        id: id.clone(),
        description,
        runtime,
        inputs,
        steps,
        outputs,
        policy,
        preconditions: Vec::new(),
        success: Vec::new(),
        failure: Vec::new(),
        recovery: Some(agentmesh_teach::RecoveryPolicy {
            max_attempts: 2,
            checkpoints,
            capture_aria: true,
            capture_screenshot: true,
            vision_adapter: None,
        }),
        observed_apis: Vec::new(),
    };
    agentmesh_teach::validate_workflow(&workflow)?;
    let path = agentmesh_teach::save_workflow(&workflow)?;
    println!(
        "Saved {id} ({} steps) to {}",
        workflow.steps.len(),
        path.display()
    );
    Ok(())
}

/// Prompts for workflow steps until an empty step id ends the loop.
fn teach_wizard_steps() -> Result<Vec<agentmesh_teach::Step>> {
    use agentmesh_teach::Step;
    let mut steps = Vec::new();
    loop {
        let id = teach_prompt("Step id (empty to finish)")?;
        if id.is_empty() {
            break;
        }
        let op = teach_prompt("Op")?;
        if !agentmesh_teach::KNOWN_OPS.contains(&op.as_str()) {
            anyhow::bail!("unknown op {op:?}; see docs/WORKFLOW_IR.md");
        }
        let target = teach_target()?;
        let value = optional(teach_prompt("Value template (optional)")?);
        let url = if op == "browser.navigate" {
            Some(teach_prompt("URL")?)
        } else {
            None
        };
        let limit = if op == "ui.extract" {
            optional(teach_prompt("Limit template (optional)")?)
        } else {
            None
        };
        steps.push(Step {
            id: id.clone(),
            op,
            target,
            value,
            url,
            limit,
            timeout_ms: None,
            condition: None,
            iterations: None,
            destination: None,
            path: None,
        });
        println!("  added step {id}");
    }
    Ok(steps)
}

/// Prompts for workflow outputs until an empty output name ends the loop.
fn teach_wizard_outputs() -> Result<std::collections::BTreeMap<String, agentmesh_teach::OutputDef>>
{
    use agentmesh_teach::OutputDef;
    let mut outputs = std::collections::BTreeMap::new();
    loop {
        let name = teach_prompt("Output name (empty to finish)")?;
        if name.is_empty() {
            break;
        }
        let from = teach_prompt("From (steps.<id>[.result])")?;
        outputs.insert(name.clone(), OutputDef { from });
        println!("  added output {name}");
    }
    Ok(outputs)
}

/// Prompts for an optional target descriptor; returns None when all blank.
fn teach_target() -> Result<Option<agentmesh_teach::Target>> {
    use agentmesh_teach::Target;
    println!("  Target (all optional; at least one for ui.* steps):");
    let semantic = optional(teach_prompt("    semantic")?);
    let role = optional(teach_prompt("    role")?);
    let accessible_name = optional(teach_prompt("    accessible name")?);
    let text = optional(teach_prompt("    visible text")?);
    let placeholder = optional(teach_prompt("    placeholder")?);
    let autocomplete = optional(teach_prompt("    autocomplete (optional)")?);
    let selectors = teach_prompt("    selectors (comma-separated)")?;
    let match_pattern = optional(teach_prompt("    match template")?);
    if semantic.is_none()
        && role.is_none()
        && accessible_name.is_none()
        && text.is_none()
        && placeholder.is_none()
        && autocomplete.is_none()
        && match_pattern.is_none()
        && selectors.trim().is_empty()
    {
        return Ok(None);
    }
    Ok(Some(Target {
        semantic,
        role,
        accessible_name,
        text,
        placeholder,
        autocomplete,
        selectors: selectors
            .split(',')
            .map(str::trim)
            .filter(|selector| !selector.is_empty())
            .map(str::to_string)
            .collect(),
        match_pattern,
    }))
}

/// Prompts for an optional workflow policy.
pub(crate) fn teach_policy() -> Result<Option<agentmesh_teach::WorkflowPolicy>> {
    if !teach_yes_no("Add a policy?", false)? {
        return Ok(None);
    }
    let origins = teach_prompt("Allowed origins (comma-separated, optional)")?;
    let max_runs = teach_prompt("Max runs per hour (optional)")?;
    let max_runs_per_hour = if max_runs.trim().is_empty() {
        None
    } else {
        Some(
            max_runs
                .trim()
                .parse::<u64>()
                .context("max runs per hour must be a positive integer")?,
        )
    };
    Ok(Some(agentmesh_teach::WorkflowPolicy {
        allowed_origins: origins
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
            .map(str::to_string)
            .collect(),
        allowed_operations: Vec::new(),
        denied_operations: Vec::new(),
        max_runs_per_hour,
    }))
}

/// Parses an input type name.
fn parse_input_type(kind: &str) -> Result<agentmesh_teach::InputType> {
    use agentmesh_teach::InputType;
    match kind.trim().to_lowercase().as_str() {
        "string" => Ok(InputType::String),
        "integer" => Ok(InputType::Integer),
        "number" => Ok(InputType::Number),
        "boolean" => Ok(InputType::Boolean),
        "array" => Ok(InputType::Array),
        "object" => Ok(InputType::Object),
        "datetime" => Ok(InputType::Datetime),
        other => anyhow::bail!("unknown input type {other:?}"),
    }
}

/// Parses a default value for an input type; empty means none.
fn teach_default_value(
    raw: &str,
    input_type: agentmesh_teach::InputType,
) -> Result<Option<serde_json::Value>> {
    use agentmesh_teach::InputType;
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let value = match input_type {
        InputType::String | InputType::Datetime => serde_json::Value::String(raw.to_string()),
        InputType::Integer => raw
            .trim()
            .parse::<i64>()
            .map(serde_json::Value::from)
            .context("default is not an integer")?,
        InputType::Number => raw
            .trim()
            .parse::<f64>()
            .map(|number| {
                serde_json::Number::from_f64(number)
                    .map_or(serde_json::Value::Null, serde_json::Value::Number)
            })
            .context("default is not a number")?,
        InputType::Boolean => match raw.trim().to_lowercase().as_str() {
            "true" | "yes" | "1" => serde_json::Value::Bool(true),
            "false" | "no" | "0" => serde_json::Value::Bool(false),
            _ => anyhow::bail!("default is not a boolean"),
        },
        InputType::Array | InputType::Object => {
            serde_json::from_str(raw).context("default is not valid JSON")?
        }
    };
    Ok(Some(value))
}

/// Reads one prompt line from the terminal.
pub(crate) fn teach_prompt(message: &str) -> Result<String> {
    eprint!("{message}: ");
    std::io::stderr()
        .flush()
        .context("failed to write prompt")?;
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .context("failed to read answer")?;
    Ok(answer.trim().to_string())
}

/// Prompts with a default used on empty answers.
pub(crate) fn teach_prompt_default(message: &str, default: &str) -> Result<String> {
    let answer = teach_prompt(&format!("{message} [{default}]"))?;
    Ok(if answer.is_empty() {
        default.to_string()
    } else {
        answer
    })
}

/// Yes/no prompt with a default.
pub(crate) fn teach_yes_no(message: &str, default: bool) -> Result<bool> {
    let hint = if default { "Y/n" } else { "y/N" };
    let answer = teach_prompt(&format!("{message} [{hint}]"))?;
    if answer.is_empty() {
        return Ok(default);
    }
    match answer.to_lowercase().as_str() {
        "y" | "yes" => Ok(true),
        "n" | "no" => Ok(false),
        _ => anyhow::bail!("answer y or n"),
    }
}

/// Empty strings become None.
pub(crate) fn optional(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

/// Lists stored workflows as a table.
fn workflows_list(lenient: bool) -> Result<()> {
    if lenient {
        let dir = agentmesh_teach::workflows_dir()?;
        let (summaries, errors) = agentmesh_teach::list_workflows_lenient(&dir);
        if summaries.is_empty() && errors.is_empty() {
            println!("No workflows stored. Run `agentmesh teach` to create one.");
            return Ok(());
        }
        println!("{:<36} {:<10} {:>5}  DESCRIPTION", "ID", "RUNTIME", "STEPS");
        for summary in summaries {
            println!(
                "{:<36} {:<10} {:>5}  {}",
                summary.id, summary.runtime, summary.steps, summary.description
            );
        }
        for error in errors {
            println!("! {}: {}", error.file, error.error);
        }
        return Ok(());
    }
    let summaries = agentmesh_teach::list_workflows()?;
    if summaries.is_empty() {
        println!("No workflows stored. Run `agentmesh teach` to create one.");
        return Ok(());
    }
    println!("{:<36} {:<10} {:>5}  DESCRIPTION", "ID", "RUNTIME", "STEPS");
    for summary in summaries {
        println!(
            "{:<36} {:<10} {:>5}  {}",
            summary.id, summary.runtime, summary.steps, summary.description
        );
    }
    Ok(())
}

/// Prints one stored workflow as YAML.
fn workflows_inspect(id: &str) -> Result<()> {
    let workflow = agentmesh_teach::load_workflow(id)?;
    println!(
        "{}",
        serde_yaml::to_string(&workflow).context("failed to render workflow")?
    );
    Ok(())
}

/// Validates a workflow file or a stored workflow id.
fn workflows_validate(target: &str) -> Result<()> {
    let workflow = if std::path::Path::new(target).is_file() {
        let document =
            std::fs::read_to_string(target).with_context(|| format!("failed to read {target}"))?;
        agentmesh_teach::parse_workflow(&document)?
    } else {
        agentmesh_teach::load_workflow(target)?
    };
    println!(
        "valid: {} ({} steps, runtime {})",
        workflow.id,
        workflow.steps.len(),
        workflow.runtime
    );
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
