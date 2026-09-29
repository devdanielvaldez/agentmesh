//! `AgentMesh` HTTP gateway and operational endpoints.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_protocol::{JsonRpcMessage, ProtocolLimits, SupportedVersions, decode_message};
use agentmesh_proxy::{
    McpProxy, MultiUpstreamProxy, ProxyBody, ProxyClient, ProxyRequest, UPSTREAM_ROUTE_HEADER,
    UpstreamEndpoint,
};
use agentmesh_transport::{
    resolve_http_protocol_version, select_response_mode, validate_json_content_type,
};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{MatchedPath, State},
    http::{HeaderMap, HeaderName, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use tower_http::{
    catch_panic::CatchPanicLayer,
    compression::CompressionLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};
use tracing::{Span, info_span};

mod pipeline;
mod policy;
mod stats;
pub use pipeline::{
    GatewayPipeline, PipelineContext, PipelineFuture, PipelineStage, RequestMiddleware,
};
pub use policy::{
    PolicyDecision, PolicyEngine, PolicyEvaluation, decision_label, redact_arguments,
};
pub use stats::{SharedStats, StatsSnapshot};

use stats::{method_label, routing_detail};

const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");
/// Internal fan-out attribution, read for accounting then stripped.
const ROUTE_HEADER: HeaderName = HeaderName::from_static(UPSTREAM_ROUTE_HEADER);

/// Builds the gateway router shared by production and tests.
pub fn router() -> Router {
    build_router(GatewayState::default())
}

/// Builds a gateway that forwards validated MCP requests to one static upstream.
pub fn router_with_upstream(proxy: ProxyClient, endpoint: UpstreamEndpoint) -> Router {
    build_router(GatewayState {
        backend: GatewayBackend::Single {
            proxy: Arc::new(proxy),
            endpoint,
        },
        stats: SharedStats::new(),
        policy: None,
    })
}

/// Builds a gateway with one static upstream and policy enforcement.
pub fn router_with_upstream_and_policy(
    proxy: ProxyClient,
    endpoint: UpstreamEndpoint,
    policy: Arc<PolicyEngine>,
) -> Router {
    build_router(GatewayState {
        backend: GatewayBackend::Single {
            proxy: Arc::new(proxy),
            endpoint,
        },
        stats: SharedStats::new(),
        policy: Some(policy),
    })
}

/// Builds a gateway that fans out validated MCP requests across static upstreams.
///
/// The proxy must be [`MultiUpstreamProxy::initialize`]d before serving traffic.
pub fn router_with_upstreams(multi: Arc<MultiUpstreamProxy>) -> Router {
    build_router(GatewayState {
        backend: GatewayBackend::Multi { proxy: multi },
        stats: SharedStats::new(),
        policy: None,
    })
}

/// Builds a fan-out gateway with policy enforcement.
///
/// The proxy must be [`MultiUpstreamProxy::initialize`]d before serving traffic.
pub fn router_with_upstreams_and_policy(
    multi: Arc<MultiUpstreamProxy>,
    policy: Arc<PolicyEngine>,
) -> Router {
    build_router(GatewayState {
        backend: GatewayBackend::Multi { proxy: multi },
        stats: SharedStats::new(),
        policy: Some(policy),
    })
}

#[derive(Clone)]
struct GatewayState {
    backend: GatewayBackend,
    stats: SharedStats,
    policy: Option<Arc<PolicyEngine>>,
}

impl Default for GatewayState {
    fn default() -> Self {
        Self {
            backend: GatewayBackend::default(),
            stats: SharedStats::new(),
            policy: None,
        }
    }
}

#[derive(Clone, Default)]
enum GatewayBackend {
    /// No upstream configured; requests fail with a safe error envelope.
    #[default]
    None,
    /// One static upstream; every validated request is forwarded to it.
    Single {
        proxy: Arc<dyn McpProxy>,
        endpoint: UpstreamEndpoint,
    },
    /// Several static upstreams with discovery-based routing.
    Multi { proxy: Arc<MultiUpstreamProxy> },
}

fn build_router(state: GatewayState) -> Router {
    Router::new()
        .route("/", get(service_info))
        .route("/health/live", get(liveness))
        .route("/health/ready", get(readiness))
        .route("/metrics", get(metrics))
        .route("/mcp", post(mcp))
        .layer(PropagateRequestIdLayer::new(REQUEST_ID_HEADER.clone()))
        .layer(SetRequestIdLayer::new(
            REQUEST_ID_HEADER.clone(),
            MakeRequestUuid,
        ))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &Request<_>| {
                    let route = request
                        .extensions()
                        .get::<MatchedPath>()
                        .map_or(request.uri().path(), MatchedPath::as_str);
                    info_span!(
                        "http.request",
                        method = %request.method(),
                        route,
                        request_id = tracing::field::Empty,
                    )
                })
                .on_request(|request: &Request<_>, span: &Span| {
                    if let Some(request_id) = request.headers().get(&REQUEST_ID_HEADER) {
                        span.record("request_id", request_id.to_str().unwrap_or("invalid"));
                    }
                })
                .on_response(
                    |response: &axum::response::Response, latency: Duration, _span: &Span| {
                        tracing::info!(status = %response.status(), latency_ms = latency.as_millis(), "request completed");
                    },
                ),
        )
        .layer(CompressionLayer::new())
        .layer(CatchPanicLayer::new())
        .with_state(state)
}

#[derive(Serialize)]
struct ServiceInfo {
    name: &'static str,
    version: &'static str,
    status: &'static str,
}

async fn service_info() -> Json<ServiceInfo> {
    Json(ServiceInfo {
        name: "agentmesh",
        version: env!("CARGO_PKG_VERSION"),
        status: "operational",
    })
}

async fn liveness() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn readiness() -> StatusCode {
    StatusCode::NO_CONTENT
}

/// Serves the live accounting snapshot for monitoring.
async fn metrics(State(state): State<GatewayState>) -> Json<StatsSnapshot> {
    Json(state.stats.snapshot())
}

async fn mcp(State(state): State<GatewayState>, headers: HeaderMap, body: Bytes) -> Response {
    let started = Instant::now();
    let mut message = match validate_mcp_http_request(&headers, &body) {
        Ok(message) => message,
        Err(error) => {
            let status = error_status(&error);
            state
                .stats
                .record("invalid", None, "-", status.as_u16(), latency_ms(started));
            return error_response(&error);
        }
    };
    let label = method_label(&message);
    let detail = routing_detail(&message);
    let mut pinned: Option<usize> = None;
    let mut policy_label: Option<String> = None;
    if let Some(engine) = &state.policy {
        match apply_policy(
            engine,
            &state.stats,
            &mut message,
            &label,
            detail.clone(),
            started,
        ) {
            Ok((pin, label)) => {
                pinned = pin;
                policy_label = label;
            }
            Err(response) => return response,
        }
    }
    forward(
        &state,
        message,
        headers,
        &label,
        detail,
        pinned,
        policy_label,
        started,
    )
    .await
}

/// Forwards one validated (and policy-processed) message to the backend.
#[allow(clippy::too_many_arguments)]
async fn forward(
    state: &GatewayState,
    message: JsonRpcMessage,
    headers: HeaderMap,
    label: &str,
    detail: Option<String>,
    pinned: Option<usize>,
    policy: Option<String>,
    started: Instant,
) -> Response {
    match &state.backend {
        GatewayBackend::Single { proxy, endpoint } => {
            if pinned.is_some_and(|target| target != 0) {
                let error = AgentMeshError::new(
                    ErrorCode::PolicyDenied,
                    "The policy route targets another upstream; configure an upstreams fan-out.",
                );
                record_error(&state.stats, label, detail, "0", &error, started, policy);
                return error_response(&error);
            }
            let request = ProxyRequest::new(endpoint.clone(), message).with_headers(headers);
            match proxy.execute(request).await {
                Ok(response) => {
                    record_response(&state.stats, label, detail, "0", response, started, policy)
                }
                Err(error) => {
                    record_error(&state.stats, label, detail, "0", &error, started, policy);
                    error_response(&error)
                }
            }
        }
        GatewayBackend::Multi { proxy } => {
            let outcome = match pinned {
                Some(target) => proxy.execute_message_pinned(message, headers, target).await,
                None => proxy.execute_message(message, headers).await,
            };
            match outcome {
                Ok(response) => {
                    record_response(&state.stats, label, detail, "?", response, started, policy)
                }
                Err(error) => {
                    record_error(&state.stats, label, detail, "?", &error, started, policy);
                    error_response(&error)
                }
            }
        }
        GatewayBackend::None => {
            let error = AgentMeshError::new(
                ErrorCode::McpProxyNotConfigured,
                "No MCP upstream is configured.",
            );
            record_error(&state.stats, label, detail, "-", &error, started, policy);
            error_response(&error)
        }
    }
}

/// Evaluates gateway policies for one validated message: applies redactions,
/// charges budgets, and resolves route pins. Returns the pin with its
/// accounting label, or the rejection response for denied calls.
fn apply_policy(
    engine: &PolicyEngine,
    stats: &SharedStats,
    message: &mut JsonRpcMessage,
    label: &str,
    detail: Option<String>,
    started: Instant,
) -> Result<(Option<usize>, Option<String>), Response> {
    let tool = if label == "tools/call" {
        detail.as_deref()
    } else {
        None
    };
    let evaluation = engine.evaluate(label, tool);
    match &evaluation.decision {
        PolicyDecision::Deny { reason, quota, .. } => {
            let code = if *quota {
                ErrorCode::QuotaExceeded
            } else {
                ErrorCode::PolicyDenied
            };
            let error = AgentMeshError::new(code, reason.clone());
            stats.record_with_policy(
                label,
                detail,
                "-",
                error_status(&error).as_u16(),
                latency_ms(started),
                Some(decision_label(engine, &evaluation.decision)),
            );
            Err(error_response(&error))
        }
        PolicyDecision::Allow { route, redact, .. } => {
            if !redact.is_empty() {
                redact_arguments(message, redact);
            }
            if let Some(slot) = evaluation.budget_slot {
                engine.consume(slot);
            }
            Ok((*route, Some(decision_label(engine, &evaluation.decision))))
        }
    }
}

/// Records a proxied response, stripping internal route attribution.
fn record_response(
    stats: &SharedStats,
    label: &str,
    detail: Option<String>,
    fallback: &str,
    response: agentmesh_proxy::ProxyResponse,
    started: Instant,
    policy: Option<String>,
) -> Response {
    let (code, mut headers, body) = response.into_parts();
    let upstream = headers
        .remove(ROUTE_HEADER)
        .and_then(|value| value.to_str().ok().map(str::to_string))
        .unwrap_or_else(|| fallback.to_string());
    stats.record_with_policy(
        label,
        detail,
        &upstream,
        code.as_u16(),
        latency_ms(started),
        policy,
    );
    render_response(code, headers, body)
}

/// Records a failed request.
fn record_error(
    stats: &SharedStats,
    label: &str,
    detail: Option<String>,
    upstream: &str,
    error: &AgentMeshError,
    started: Instant,
    policy: Option<String>,
) {
    stats.record_with_policy(
        label,
        detail,
        upstream,
        error_status(error).as_u16(),
        latency_ms(started),
        policy,
    );
}

/// Saturating millisecond latency since request start.
fn latency_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn validate_mcp_http_request(
    headers: &HeaderMap,
    body: &[u8],
) -> Result<JsonRpcMessage, AgentMeshError> {
    validate_json_content_type(headers)?;
    let message = decode_message(body, ProtocolLimits::default())?;
    let supported = SupportedVersions::latest_only();
    resolve_http_protocol_version(headers, &message, &supported)?;
    select_response_mode(headers)?;
    Ok(message)
}

/// Renders a proxied response with already-accounted headers.
fn render_response(
    status: StatusCode,
    headers: HeaderMap,
    body: agentmesh_proxy::ProxyBody,
) -> Response {
    let mut response = match body {
        ProxyBody::Empty => Body::empty().into_response(),
        ProxyBody::Json(message) => Json(message).into_response(),
        ProxyBody::ServerSentEvents(stream) => Body::from_stream(stream).into_response(),
    };
    *response.status_mut() = status;
    response.headers_mut().extend(headers);
    response
}

/// Maps an error to its public HTTP status.
fn error_status(error: &AgentMeshError) -> StatusCode {
    StatusCode::from_u16(error.code().http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

fn error_response(error: &AgentMeshError) -> Response {
    let status = error_status(error);
    (status, Json(error.public_response())).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentmesh_config::{GatewayConfig, PolicyMatch, PolicyRule, UpstreamConfig};
    use agentmesh_proxy::ProxyConfig;
    use axum::body::{Body, to_bytes};
    use tower::ServiceExt;

    /// Builds a single-upstream gateway whose only policy denies `secret.*`.
    /// Denied calls never reach the (unroutable) dummy upstream.
    fn deny_gateway() -> Router {
        let gateway = GatewayConfig {
            environment: "test".to_string(),
            upstream: Some(UpstreamConfig {
                url: "http://127.0.0.1:9/mcp".to_string(),
                name: Some("only".to_string()),
                allow_insecure_http: true,
                request_timeout_ms: 1_000,
            }),
            policies: vec![PolicyRule {
                match_spec: PolicyMatch {
                    tool: Some("secret.*".to_string()),
                    method: None,
                    environment: None,
                },
                deny: Some("Blocked by policy.".to_string()),
                route: None,
                budget: None,
                redact: Vec::new(),
            }],
            ..GatewayConfig::default()
        };
        let engine = Arc::new(PolicyEngine::new(&gateway).expect("engine builds"));
        let proxy = ProxyClient::new(ProxyConfig::default()).expect("proxy client");
        let endpoint =
            UpstreamEndpoint::parse("http://127.0.0.1:9/mcp", true).expect("test endpoint");
        router_with_upstream_and_policy(proxy, endpoint, engine)
    }

    fn tools_call_body(tool: &str) -> Body {
        Body::from(format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{tool}","arguments":{{}},"_meta":{{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{{}}}}}}}}"#
        ))
    }

    #[tokio::test]
    async fn policy_deny_rejects_before_proxying() {
        let app = deny_gateway();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .header("accept", "application/json")
                    .header("mcp-protocol-version", "2026-07-28")
                    .body(tools_call_body("secret.read"))
                    .expect("valid request"),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("read response body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON response");
        assert_eq!(body["error"]["code"], "POLICY_DENIED");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router response");
        let body = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("read metrics body");
        let metrics: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON metrics");
        let recent = metrics["recent"].as_array().expect("recent events");
        let denied = recent
            .iter()
            .find(|event| event["status"] == 403)
            .expect("denied event");
        assert_eq!(denied["policy"], "deny:rule0");
    }

    #[tokio::test]
    async fn liveness_is_successful() {
        let response = router()
            .oneshot(
                Request::builder()
                    .uri("/health/live")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(response.headers().contains_key(&REQUEST_ID_HEADER));
    }

    #[tokio::test]
    async fn mcp_errors_use_the_shared_public_envelope() {
        let body = Body::from(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
        );
        let response = router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .header("accept", "application/json")
                    .header("mcp-protocol-version", "2026-07-28")
                    .body(body)
                    .expect("valid request"),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("read response body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON response");
        assert_eq!(body["error"]["code"], "MCP_PROXY_NOT_CONFIGURED");
        assert_eq!(body["error"]["retryable"], false);
    }

    #[tokio::test]
    async fn mcp_rejects_invalid_transport_binding_before_proxying() {
        let response = router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("content-type", "text/plain")
                    .body(Body::from("not MCP"))
                    .expect("valid HTTP request"),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("read response body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON response");
        assert_eq!(body["error"]["code"], "INVALID_REQUEST");
    }

    #[tokio::test]
    async fn metrics_reports_served_requests() {
        let app = router();
        let mcp_body = Body::from(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
        );
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .header("accept", "application/json")
                    .header("mcp-protocol-version", "2026-07-28")
                    .body(mcp_body)
                    .expect("valid request"),
            )
            .await
            .expect("router response");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("read response body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON response");
        assert_eq!(body["requests"], 1);
        assert_eq!(body["methods"]["tools/list"]["requests"], 1);
        assert_eq!(body["recent"].as_array().expect("recent array").len(), 1);
    }
}
