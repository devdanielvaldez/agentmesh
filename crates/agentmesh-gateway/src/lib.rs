//! `AgentMesh` HTTP gateway and operational endpoints.

use std::{sync::Arc, time::Duration};

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_protocol::{JsonRpcMessage, ProtocolLimits, SupportedVersions, decode_message};
use agentmesh_proxy::{
    McpProxy, MultiUpstreamProxy, ProxyBody, ProxyClient, ProxyRequest, UpstreamEndpoint,
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
pub use pipeline::{
    GatewayPipeline, PipelineContext, PipelineFuture, PipelineStage, RequestMiddleware,
};

const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

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
    })
}

/// Builds a gateway that fans out validated MCP requests across static upstreams.
///
/// The proxy must be [`MultiUpstreamProxy::initialize`]d before serving traffic.
pub fn router_with_upstreams(multi: Arc<MultiUpstreamProxy>) -> Router {
    build_router(GatewayState {
        backend: GatewayBackend::Multi { proxy: multi },
    })
}

#[derive(Clone, Default)]
struct GatewayState {
    backend: GatewayBackend,
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

async fn mcp(State(state): State<GatewayState>, headers: HeaderMap, body: Bytes) -> Response {
    let message = match validate_mcp_http_request(&headers, &body) {
        Ok(message) => message,
        Err(error) => return error_response(&error),
    };
    match &state.backend {
        GatewayBackend::Single { proxy, endpoint } => {
            let request = ProxyRequest::new(endpoint.clone(), message).with_headers(headers);
            match proxy.execute(request).await {
                Ok(response) => proxy_response(response),
                Err(error) => error_response(&error),
            }
        }
        GatewayBackend::Multi { proxy } => match proxy.execute_message(message, headers).await {
            Ok(response) => proxy_response(response),
            Err(error) => error_response(&error),
        },
        GatewayBackend::None => error_response(&AgentMeshError::new(
            ErrorCode::McpProxyNotConfigured,
            "No MCP upstream is configured.",
        )),
    }
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

fn proxy_response(response: agentmesh_proxy::ProxyResponse) -> Response {
    let (status, headers, body) = response.into_parts();
    let mut response = match body {
        ProxyBody::Empty => Body::empty().into_response(),
        ProxyBody::Json(message) => Json(message).into_response(),
        ProxyBody::ServerSentEvents(stream) => Body::from_stream(stream).into_response(),
    };
    *response.status_mut() = status;
    response.headers_mut().extend(headers);
    response
}

fn error_response(error: &AgentMeshError) -> Response {
    let status = StatusCode::from_u16(error.code().http_status())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(error.public_response())).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use tower::ServiceExt;

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
}
