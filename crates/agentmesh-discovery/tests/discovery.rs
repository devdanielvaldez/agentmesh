//! Capability discovery, reconciliation, and provider conformance tests.

use std::{collections::VecDeque, sync::Mutex};

use agentmesh_core::Transport;
use agentmesh_discovery::{
    ConservativePolicy, DiscoveryClient, DiscoveryDisposition, DiscoveryEngine, DiscoveryFuture,
    DiscoveryLimits, EndpointCandidate, EndpointProvider, ProxyDiscoveryClient, StaticProvider,
};
use agentmesh_error::ErrorCode;
use agentmesh_protocol::McpMethod;
use agentmesh_proxy::{ProxyClient, ProxyConfig, UpstreamEndpoint};
use agentmesh_registry::{
    InMemoryRegistry, RegisteredService, Registry, RegistryScope, ServiceLifecycle, WriteCondition,
};
use axum::{Json, Router, http::HeaderMap, routing::post};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};

struct FakeClient {
    pages: Mutex<VecDeque<(McpMethod, Option<String>, Value)>>,
}

impl FakeClient {
    fn new(pages: Vec<(McpMethod, Option<&str>, Value)>) -> Self {
        Self {
            pages: Mutex::new(
                pages
                    .into_iter()
                    .map(|(method, cursor, value)| (method, cursor.map(str::to_owned), value))
                    .collect(),
            ),
        }
    }
}

impl DiscoveryClient for FakeClient {
    fn list_page(&self, method: McpMethod, cursor: Option<String>) -> DiscoveryFuture<'_> {
        Box::pin(async move {
            let (expected_method, expected_cursor, result) = self
                .pages
                .lock()
                .expect("fake client lock")
                .pop_front()
                .expect("expected discovery call");
            assert_eq!(method, expected_method);
            assert_eq!(cursor, expected_cursor);
            Ok(result)
        })
    }
}

fn catalog_pages(tool_description: &str) -> Vec<(McpMethod, Option<&str>, Value)> {
    vec![
        (
            McpMethod::ServerDiscover,
            None,
            json!({
                "resultType": "complete",
                "supportedVersions": ["2026-07-28"],
                "capabilities": {"tools": {}, "resources": {}, "prompts": {}}
            }),
        ),
        (
            McpMethod::ToolsList,
            None,
            json!({
                "tools": [{
                    "name": "weather.current",
                    "description": tool_description,
                    "inputSchema": {"type": "object"}
                }],
                "nextCursor": "tools-2",
                "ttlMs": 30_000,
                "cacheScope": "private"
            }),
        ),
        (
            McpMethod::ToolsList,
            Some("tools-2"),
            json!({
                "tools": [{"name": "weather.forecast", "inputSchema": {"type": "object"}}],
                "ttlMs": 30_000,
                "cacheScope": "private"
            }),
        ),
        (
            McpMethod::ResourcesList,
            None,
            json!({"resources": [{"uri": "weather://stations", "name": "Stations"}]}),
        ),
        (
            McpMethod::ResourcesTemplatesList,
            None,
            json!({"resourceTemplates": [{"uriTemplate": "weather://stations/{id}"}]}),
        ),
        (
            McpMethod::PromptsList,
            None,
            json!({"prompts": [{"name": "weather.summary", "arguments": []}]}),
        ),
    ]
}

fn engine(description: &str) -> DiscoveryEngine<FakeClient, ConservativePolicy> {
    DiscoveryEngine::new(
        FakeClient::new(catalog_pages(description)),
        ConservativePolicy,
        DiscoveryLimits::default(),
    )
    .expect("discovery engine")
}

#[tokio::test]
async fn discovery_follows_pagination_and_normalizes_capabilities() {
    let catalog = engine("Current weather")
        .discover()
        .await
        .expect("discover catalog");

    assert_eq!(catalog.pages, 5);
    assert_eq!(catalog.capabilities.len(), 5);
    assert_eq!(catalog.capabilities[0].name, "weather.current");
    assert!(catalog.fingerprint.starts_with("sha256:"));
    assert_eq!(catalog.fingerprint.len(), 71);
    assert_eq!(catalog.cache_hints[0].ttl_ms, Some(30_000));
}

#[tokio::test]
async fn conservative_reconciliation_accepts_bootstrap_then_quarantines_change() {
    let registry = InMemoryRegistry::new();
    let scope = RegistryScope::new("acme", "production").expect("scope");
    let service = registry
        .put(
            RegisteredService::new(scope.clone(), "weather"),
            WriteCondition::Create,
        )
        .expect("register service");

    let first = engine("Current weather")
        .reconcile(&registry, &scope, service.id)
        .await
        .expect("initial discovery");
    assert_eq!(first.disposition, DiscoveryDisposition::Accepted);
    assert_eq!(first.service_revision.get(), 2);

    let changed = engine("Current conditions")
        .reconcile(&registry, &scope, service.id)
        .await
        .expect("changed discovery");
    assert_eq!(changed.disposition, DiscoveryDisposition::Quarantined);
    assert_eq!(changed.difference.changed.len(), 1);

    let stored = registry
        .get(&scope, service.id)
        .expect("registry read")
        .expect("stored service");
    assert_eq!(stored.lifecycle, ServiceLifecycle::Quarantined);
    assert_eq!(stored.revision.get(), 3);

    let unchanged = engine("Current conditions")
        .reconcile(&registry, &scope, service.id)
        .await
        .expect("unchanged discovery");
    assert_eq!(unchanged.disposition, DiscoveryDisposition::Unchanged);
    assert_eq!(unchanged.service_revision.get(), 3);
}

#[tokio::test]
async fn discovery_rejects_repeated_pagination_cursors() {
    let client = FakeClient::new(vec![
        (
            McpMethod::ServerDiscover,
            None,
            json!({
                "resultType": "complete",
                "supportedVersions": ["2026-07-28"],
                "capabilities": {"tools": {}}
            }),
        ),
        (
            McpMethod::ToolsList,
            None,
            json!({"tools": [], "nextCursor": "repeat"}),
        ),
        (
            McpMethod::ToolsList,
            Some("repeat"),
            json!({"tools": [], "nextCursor": "repeat"}),
        ),
    ]);
    let engine = DiscoveryEngine::new(client, ConservativePolicy, DiscoveryLimits::default())
        .expect("engine");

    let error = engine.discover().await.expect_err("cursor loop");
    assert_eq!(error.code(), ErrorCode::UpstreamProtocolError);
}

#[tokio::test]
async fn discovery_calls_only_capability_lists_advertised_by_the_server() {
    let client = FakeClient::new(vec![
        (
            McpMethod::ServerDiscover,
            None,
            json!({
                "resultType": "complete",
                "supportedVersions": ["2026-07-28"],
                "capabilities": {"tools": {}}
            }),
        ),
        (
            McpMethod::ToolsList,
            None,
            json!({"tools": [{"name": "weather.current", "inputSchema": {}}]}),
        ),
    ]);
    let engine = DiscoveryEngine::new(client, ConservativePolicy, DiscoveryLimits::default())
        .expect("engine");

    let catalog = engine.discover().await.expect("catalog");
    assert_eq!(catalog.pages, 1);
    assert_eq!(catalog.capabilities.len(), 1);
    assert_eq!(catalog.cache_hints.len(), 1);
}

#[tokio::test]
async fn discovery_rejects_servers_without_a_supported_modern_version() {
    let client = FakeClient::new(vec![(
        McpMethod::ServerDiscover,
        None,
        json!({
            "resultType": "complete",
            "supportedVersions": ["2025-11-25"],
            "capabilities": {}
        }),
    )]);
    let engine = DiscoveryEngine::new(client, ConservativePolicy, DiscoveryLimits::default())
        .expect("engine");

    let error = engine.discover().await.expect_err("unsupported version");
    assert_eq!(error.code(), ErrorCode::UnsupportedProtocolVersion);
}

#[tokio::test]
async fn static_provider_returns_bounded_candidates() {
    let candidate = EndpointCandidate {
        transport: Transport::StreamableHttp,
        target: "https://weather.example.com/mcp".into(),
        labels: [("region".into(), "us-east".into())].into(),
    };
    let scope = RegistryScope::new("acme", "production").expect("scope");
    let provider =
        StaticProvider::new(scope.clone(), vec![candidate.clone()]).expect("static provider");

    assert_eq!(
        provider.discover(scope.clone()).await.expect("candidates"),
        vec![candidate]
    );
    assert!(
        provider
            .discover(RegistryScope::new("other", "production").expect("other scope"))
            .await
            .expect("isolated candidates")
            .is_empty()
    );
}

#[tokio::test]
async fn proxy_client_discovers_a_real_streamable_http_server() {
    async fn upstream(headers: HeaderMap, Json(request): Json<Value>) -> Json<Value> {
        assert_eq!(
            headers
                .get("mcp-protocol-version")
                .and_then(|value| value.to_str().ok()),
            Some(agentmesh_protocol::LATEST_PROTOCOL_VERSION)
        );
        let method = request["method"].as_str().expect("method");
        let result = match method {
            "server/discover" => json!({
                "resultType": "complete",
                "supportedVersions": ["2026-07-28"],
                "capabilities": {"tools": {}, "resources": {}, "prompts": {}}
            }),
            "tools/list" => json!({"tools": [{"name": "weather.current", "inputSchema": {}}]}),
            "resources/list" => json!({"resources": []}),
            "resources/templates/list" => json!({"resourceTemplates": []}),
            "prompts/list" => json!({"prompts": []}),
            _ => panic!("unexpected discovery method"),
        };
        Json(json!({"jsonrpc": "2.0", "id": request["id"], "result": result}))
    }

    let (url, server) = start_server(Router::new().route("/mcp", post(upstream))).await;
    let endpoint = UpstreamEndpoint::parse(&url, true).expect("local endpoint");
    let client = ProxyDiscoveryClient::new(
        ProxyClient::new(ProxyConfig::default()).expect("proxy client"),
        endpoint,
    );
    let engine = DiscoveryEngine::new(client, ConservativePolicy, DiscoveryLimits::default())
        .expect("engine");

    let catalog = engine.discover().await.expect("HTTP discovery");
    assert_eq!(catalog.capabilities.len(), 1);
    assert_eq!(catalog.capabilities[0].name, "weather.current");
    server.abort();
}

async fn start_server(app: Router) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let address = listener.local_addr().expect("local address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve test app");
    });
    (format!("http://{address}/mcp"), server)
}
