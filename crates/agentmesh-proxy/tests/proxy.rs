//! Upstream validation, header isolation, and forwarding tests.

use agentmesh_error::ErrorCode;
use agentmesh_protocol::{JsonRpcMessage, JsonRpcRequest, McpMethod, RequestId};
use agentmesh_proxy::{
    McpProxy, ProxyBody, ProxyClient, ProxyConfig, ProxyRequest, UpstreamCredential,
    UpstreamEndpoint, filter_request_headers,
};
use axum::{Router, body::Bytes, http::HeaderMap, response::Response, routing::post};
use http::{HeaderValue, StatusCode, header};
use serde_json::json;
use tokio::{net::TcpListener, task::JoinHandle};

fn ping() -> JsonRpcMessage {
    JsonRpcMessage::Request(JsonRpcRequest::new(
        RequestId::Integer(1),
        McpMethod::Ping,
        Some(json!({})),
    ))
}

#[test]
fn endpoint_requires_https_unless_http_is_explicitly_allowed() {
    assert!(UpstreamEndpoint::parse("https://example.com/mcp", false).is_ok());
    assert!(UpstreamEndpoint::parse("http://127.0.0.1:3000/mcp", false).is_err());
    assert!(UpstreamEndpoint::parse("http://127.0.0.1:3000/mcp", true).is_ok());
    assert!(UpstreamEndpoint::parse("https://user:secret@example.com/mcp", false).is_err());
    assert!(UpstreamEndpoint::parse("https://example.com/mcp#secret", false).is_err());
}

#[test]
fn endpoint_and_credentials_redact_sensitive_values() {
    let endpoint = UpstreamEndpoint::parse("https://example.com/secret-path?token=secret", false)
        .expect("valid endpoint");
    let credential = UpstreamCredential::bearer("secret-token").expect("valid bearer token");

    let endpoint_debug = format!("{endpoint:?}");
    let credential_debug = format!("{credential:?}");
    assert!(!endpoint_debug.contains("secret"));
    assert!(!credential_debug.contains("secret-token"));
    assert!(endpoint_debug.contains("has_query"));
    assert!(credential_debug.contains("[REDACTED]"));
}

#[test]
fn request_debug_redacts_message_contents_and_header_values() {
    let endpoint =
        UpstreamEndpoint::parse("https://example.com/mcp", false).expect("valid endpoint");
    let mut headers = HeaderMap::new();
    headers.insert("x-test-secret", HeaderValue::from_static("header-secret"));
    let message = JsonRpcMessage::Request(JsonRpcRequest::new(
        RequestId::Integer(1),
        McpMethod::Ping,
        Some(json!({"token": "message-secret"})),
    ));

    let debug = format!(
        "{:?}",
        ProxyRequest::new(endpoint, message).with_headers(headers)
    );
    assert!(!debug.contains("message-secret"));
    assert!(!debug.contains("header-secret"));
    assert!(debug.contains("x-test-secret"));
}

#[test]
fn caller_credentials_and_hop_headers_are_not_forwarded() {
    let mut source = HeaderMap::new();
    source.insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer caller"),
    );
    source.insert(header::COOKIE, HeaderValue::from_static("session=caller"));
    source.insert(header::HOST, HeaderValue::from_static("gateway.example"));
    source.insert(
        "mcp-protocol-version",
        HeaderValue::from_static("2026-07-28"),
    );
    source.insert("traceparent", HeaderValue::from_static("00-trace-parent"));

    let filtered = filter_request_headers(&source);
    assert!(!filtered.contains_key(header::AUTHORIZATION));
    assert!(!filtered.contains_key(header::COOKIE));
    assert!(!filtered.contains_key(header::HOST));
    assert!(filtered.contains_key("mcp-protocol-version"));
    assert!(filtered.contains_key("traceparent"));
}

#[tokio::test]
async fn proxy_forwards_json_with_only_the_trusted_credential() {
    async fn upstream(headers: HeaderMap, _body: Bytes) -> Response<String> {
        let trusted = headers
            .get("x-upstream-key")
            .and_then(|value| value.to_str().ok());
        if headers.contains_key(header::AUTHORIZATION)
            || trusted != Some("trusted-secret")
            || headers.get("mcp-method") != Some(&HeaderValue::from_static("ping"))
        {
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(String::new())
                .expect("valid response");
        }
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#.into())
            .expect("valid response")
    }

    let (url, server) = start_server(Router::new().route("/mcp", post(upstream))).await;
    let endpoint = UpstreamEndpoint::parse(&url, true).expect("local HTTP endpoint");
    let mut caller_headers = HeaderMap::new();
    caller_headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer caller-secret"),
    );
    let credential = UpstreamCredential::header("x-upstream-key", "trusted-secret")
        .expect("valid upstream credential");
    let request = ProxyRequest::new(endpoint, ping())
        .with_headers(caller_headers)
        .with_credential(credential);

    let response = ProxyClient::new(ProxyConfig::default())
        .expect("proxy client")
        .execute(request)
        .await
        .expect("successful upstream response");

    assert_eq!(response.status(), StatusCode::OK);
    assert!(matches!(response.into_parts().2, ProxyBody::Json(_)));
    server.abort();
}

#[tokio::test]
async fn proxy_enforces_the_response_size_limit() {
    async fn upstream() -> Response<String> {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(r#"{"jsonrpc":"2.0","id":1,"result":{"large":"payload"}}"#.into())
            .expect("valid response")
    }

    let (url, server) = start_server(Router::new().route("/mcp", post(upstream))).await;
    let endpoint = UpstreamEndpoint::parse(&url, true).expect("local HTTP endpoint");
    let proxy = ProxyClient::new(ProxyConfig {
        max_response_bytes: 16,
        ..ProxyConfig::default()
    })
    .expect("proxy client");

    let error = proxy
        .execute(ProxyRequest::new(endpoint, ping()))
        .await
        .expect_err("oversized response");

    assert_eq!(error.code(), ErrorCode::PayloadTooLarge);
    server.abort();
}

#[tokio::test]
async fn proxy_rejects_a_response_with_the_wrong_request_id() {
    async fn upstream() -> Response<String> {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(r#"{"jsonrpc":"2.0","id":2,"result":{}}"#.into())
            .expect("valid response")
    }

    let (url, server) = start_server(Router::new().route("/mcp", post(upstream))).await;
    let endpoint = UpstreamEndpoint::parse(&url, true).expect("local HTTP endpoint");
    let error = ProxyClient::new(ProxyConfig::default())
        .expect("proxy client")
        .execute(ProxyRequest::new(endpoint, ping()))
        .await
        .expect_err("mismatched response id");

    assert_eq!(error.code(), ErrorCode::UpstreamProtocolError);
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
