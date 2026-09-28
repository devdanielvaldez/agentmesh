//! Pooled HTTP implementation of the MCP upstream proxy.

use std::{future::Future, pin::Pin, time::Duration};

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_protocol::{JsonRpcMessage, McpMethod, ProtocolLimits, RequestId, decode_message};
use bytes::{Bytes, BytesMut};
use futures_util::{StreamExt, TryStreamExt};
use http::{HeaderValue, StatusCode, header};

use crate::{
    ProxyBody, ProxyRequest, ProxyResponse, ProxyStream, filter_request_headers,
    filter_response_headers,
};

/// Future returned by object-safe proxy implementations.
pub type ProxyFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ProxyResponse, AgentMeshError>> + Send + 'a>>;

/// Object-safe MCP upstream forwarding contract.
pub trait McpProxy: Send + Sync {
    /// Executes one upstream request.
    fn execute(&self, request: ProxyRequest) -> ProxyFuture<'_>;
}

/// Connection, deadline, and response-bound configuration.
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// TCP/TLS connection establishment timeout.
    pub connect_timeout: Duration,
    /// Default complete request timeout.
    pub request_timeout: Duration,
    /// Maximum cumulative bytes accepted from one upstream response.
    pub max_response_bytes: usize,
    /// HTTP user agent sent by the shared client.
    pub user_agent: String,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(2),
            request_timeout: Duration::from_secs(30),
            max_response_bytes: 50 * 1024 * 1024,
            user_agent: format!("agentmesh/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

/// Pooled `reqwest` implementation with redirects disabled.
#[derive(Clone)]
pub struct ProxyClient {
    client: reqwest::Client,
    config: ProxyConfig,
}

impl ProxyClient {
    /// Builds a pooled client from validated proxy configuration.
    ///
    /// Redirects are disabled so upstream routing and future SSRF policy cannot
    /// be bypassed by a server-controlled redirect.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] when the underlying HTTP client cannot be built.
    pub fn new(config: ProxyConfig) -> Result<Self, AgentMeshError> {
        if config.max_response_bytes == 0
            || config.connect_timeout.is_zero()
            || config.request_timeout.is_zero()
        {
            return Err(AgentMeshError::new(
                ErrorCode::ConfigurationInvalid,
                "Proxy limits and timeouts must be greater than zero.",
            ));
        }
        let client = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(&config.user_agent)
            .build()
            .map_err(|error| {
                AgentMeshError::with_source(
                    ErrorCode::ConfigurationInvalid,
                    "The MCP proxy HTTP client could not be initialized.",
                    error,
                )
            })?;
        Ok(Self { client, config })
    }

    async fn execute_request(
        &self,
        request: ProxyRequest,
    ) -> Result<ProxyResponse, AgentMeshError> {
        let (expected_id, method, name) = match &request.message {
            JsonRpcMessage::Request(message) => (
                Some(message.id.clone()),
                message.method.clone(),
                routing_name(&message.method, message.params.as_ref()),
            ),
            JsonRpcMessage::Notification(message) => (
                None,
                message.method.clone(),
                routing_name(&message.method, message.params.as_ref()),
            ),
            JsonRpcMessage::Response(_) => {
                return Err(AgentMeshError::new(
                    ErrorCode::InvalidRequest,
                    "An MCP response cannot be forwarded as an upstream request.",
                ));
            }
        };
        let body = serde_json::to_vec(&request.message).map_err(|error| {
            AgentMeshError::with_source(
                ErrorCode::Internal,
                "The MCP request could not be encoded.",
                error,
            )
        })?;
        let protocol_limits = ProtocolLimits::default();
        if body.len() > protocol_limits.max_message_bytes {
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "The MCP request exceeds the configured size limit.",
            ));
        }

        let headers =
            upstream_headers(&request.headers, &method, name, request.credential.as_ref())?;

        let response = self
            .client
            .post(request.endpoint.as_url().clone())
            .headers(headers)
            .timeout(request.timeout.unwrap_or(self.config.request_timeout))
            .body(body)
            .send()
            .await
            .map_err(map_reqwest_error)?;

        let status = response.status();
        if !status.is_success() {
            return Err(map_status(status));
        }
        let headers = filter_response_headers(response.headers());
        if matches!(status, StatusCode::NO_CONTENT | StatusCode::ACCEPTED) {
            return Ok(ProxyResponse::new(status, headers, ProxyBody::Empty));
        }

        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .ok_or_else(invalid_upstream_content_type)?;

        match content_type.as_str() {
            "application/json" => {
                let body = collect_bounded(response, self.config.max_response_bytes).await?;
                let message = decode_message(
                    &body,
                    ProtocolLimits {
                        max_message_bytes: self.config.max_response_bytes,
                        ..protocol_limits
                    },
                )
                .map_err(invalid_upstream_message)?;
                validate_upstream_message(expected_id.as_ref(), &message)?;
                Ok(ProxyResponse::new(
                    status,
                    headers,
                    ProxyBody::Json(message),
                ))
            }
            "text/event-stream" => {
                let stream = bounded_stream(response, self.config.max_response_bytes);
                Ok(ProxyResponse::new(
                    status,
                    headers,
                    ProxyBody::ServerSentEvents(stream),
                ))
            }
            _ => Err(invalid_upstream_content_type()),
        }
    }
}

fn upstream_headers(
    caller_headers: &http::HeaderMap,
    method: &McpMethod,
    name: Option<&str>,
    credential: Option<&crate::UpstreamCredential>,
) -> Result<http::HeaderMap, AgentMeshError> {
    let mut headers = filter_request_headers(caller_headers);
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(
        header::ACCEPT,
        HeaderValue::from_static("application/json, text/event-stream"),
    );
    headers.insert(
        "mcp-method",
        HeaderValue::from_str(method.as_str()).map_err(invalid_routing_header)?,
    );
    if let Some(name) = name {
        headers.insert(
            "mcp-name",
            HeaderValue::from_str(name).map_err(invalid_routing_header)?,
        );
    }
    if let Some(credential) = credential {
        let (name, value) = credential.parts();
        headers.insert(name.clone(), value.clone());
    }
    Ok(headers)
}

fn routing_name<'a>(method: &McpMethod, params: Option<&'a serde_json::Value>) -> Option<&'a str> {
    let key = match method {
        McpMethod::ToolsCall | McpMethod::PromptsGet => "name",
        McpMethod::ResourcesRead
        | McpMethod::ResourcesSubscribe
        | McpMethod::ResourcesUnsubscribe => "uri",
        McpMethod::TasksGet | McpMethod::TasksCancel => "taskId",
        _ => return None,
    };
    params?.get(key)?.as_str()
}

fn invalid_routing_header(source: http::header::InvalidHeaderValue) -> AgentMeshError {
    AgentMeshError::with_source(
        ErrorCode::InvalidRequest,
        "The MCP method or capability name cannot be represented as an HTTP header.",
        source,
    )
}

fn validate_upstream_message(
    expected_id: Option<&RequestId>,
    message: &JsonRpcMessage,
) -> Result<(), AgentMeshError> {
    match (expected_id, message) {
        (Some(expected), JsonRpcMessage::Response(response)) if response.id() == expected => Ok(()),
        _ => Err(AgentMeshError::new(
            ErrorCode::UpstreamProtocolError,
            "The MCP upstream returned an unexpected JSON-RPC message.",
        )),
    }
}

fn invalid_upstream_message(source: AgentMeshError) -> AgentMeshError {
    AgentMeshError::with_source(
        ErrorCode::UpstreamProtocolError,
        "The MCP upstream returned an invalid JSON-RPC message.",
        source,
    )
}

impl McpProxy for ProxyClient {
    fn execute(&self, request: ProxyRequest) -> ProxyFuture<'_> {
        Box::pin(async move { self.execute_request(request).await })
    }
}

async fn collect_bounded(
    response: reqwest::Response,
    maximum: usize,
) -> Result<Bytes, AgentMeshError> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(response_too_large());
    }
    let mut body = BytesMut::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.try_next().await.map_err(map_reqwest_error)? {
        if body.len().saturating_add(chunk.len()) > maximum {
            return Err(response_too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body.freeze())
}

fn bounded_stream(response: reqwest::Response, maximum: usize) -> ProxyStream {
    let mut received = 0_usize;
    let mut exceeded = false;
    let stream = response.bytes_stream().map(move |item| {
        if exceeded {
            return Err(response_too_large());
        }
        let chunk = item.map_err(map_reqwest_error)?;
        received = received.saturating_add(chunk.len());
        if received > maximum {
            exceeded = true;
            Err(response_too_large())
        } else {
            Ok(chunk)
        }
    });
    Box::pin(stream)
}

fn map_reqwest_error(source: reqwest::Error) -> AgentMeshError {
    let (code, message) = if source.is_timeout() {
        (
            ErrorCode::UpstreamTimeout,
            "The MCP upstream request timed out.",
        )
    } else {
        (
            ErrorCode::UpstreamConnectionFailed,
            "The MCP upstream connection failed.",
        )
    };
    AgentMeshError::with_source(code, message, source)
}

fn map_status(status: StatusCode) -> AgentMeshError {
    let (code, message) = match status {
        StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => (
            ErrorCode::UpstreamTimeout,
            "The MCP upstream request timed out.",
        ),
        StatusCode::TOO_MANY_REQUESTS => (
            ErrorCode::RateLimited,
            "The MCP upstream rate limit was exceeded.",
        ),
        status if status.is_server_error() => (
            ErrorCode::UpstreamUnavailable,
            "The MCP upstream is temporarily unavailable.",
        ),
        _ => (
            ErrorCode::UpstreamProtocolError,
            "The MCP upstream rejected the request.",
        ),
    };
    AgentMeshError::new(code, message)
}

fn invalid_upstream_content_type() -> AgentMeshError {
    AgentMeshError::new(
        ErrorCode::UpstreamProtocolError,
        "The MCP upstream returned an unsupported content type.",
    )
}

fn response_too_large() -> AgentMeshError {
    AgentMeshError::new(
        ErrorCode::PayloadTooLarge,
        "The MCP upstream response exceeds the configured size limit.",
    )
}
