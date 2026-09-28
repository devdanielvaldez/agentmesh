//! MCP list-call client contract and HTTP proxy adapter.

use std::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicI64, Ordering},
};

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_protocol::{
    CapabilitySet, JsonRpcMessage, JsonRpcRequest, JsonRpcResponse, McpMethod, ProtocolVersion,
    RequestId, RequestMeta,
};
use agentmesh_proxy::{
    McpProxy, ProxyBody, ProxyClient, ProxyRequest, UpstreamCredential, UpstreamEndpoint,
};
use http::{HeaderMap, HeaderValue};
use serde_json::{Map, Value};

/// Future returned by object-safe capability discovery clients.
pub type DiscoveryFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Value, AgentMeshError>> + Send + 'a>>;

/// Executes one MCP discovery or paginated list call and returns its result object.
pub trait DiscoveryClient: Send + Sync {
    /// Calls a discovery method with an optional opaque list cursor.
    fn list_page(&self, method: McpMethod, cursor: Option<String>) -> DiscoveryFuture<'_>;
}

/// Discovery client backed by the credential-safe MCP HTTP proxy.
pub struct ProxyDiscoveryClient {
    proxy: ProxyClient,
    endpoint: UpstreamEndpoint,
    credential: Option<UpstreamCredential>,
    next_id: AtomicI64,
}

impl ProxyDiscoveryClient {
    /// Creates a client for one validated upstream endpoint.
    pub const fn new(proxy: ProxyClient, endpoint: UpstreamEndpoint) -> Self {
        Self {
            proxy,
            endpoint,
            credential: None,
            next_id: AtomicI64::new(1),
        }
    }

    /// Adds a trusted upstream credential to every discovery request.
    #[must_use]
    pub fn with_credential(mut self, credential: UpstreamCredential) -> Self {
        self.credential = Some(credential);
        self
    }

    fn request_id(&self) -> Result<RequestId, AgentMeshError> {
        self.next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map(RequestId::Integer)
            .map_err(|_| {
                AgentMeshError::new(
                    ErrorCode::Internal,
                    "The discovery request identifier space is exhausted.",
                )
            })
    }
}

impl DiscoveryClient for ProxyDiscoveryClient {
    fn list_page(&self, method: McpMethod, cursor: Option<String>) -> DiscoveryFuture<'_> {
        Box::pin(async move {
            let mut parameters = Map::new();
            if let Some(cursor) = cursor {
                parameters.insert("cursor".into(), Value::String(cursor));
            }
            let metadata = RequestMeta::new(ProtocolVersion::latest(), CapabilitySet::new());
            parameters.insert(
                "_meta".into(),
                serde_json::to_value(metadata).map_err(encoding_error)?,
            );
            let message = JsonRpcMessage::Request(JsonRpcRequest::new(
                self.request_id()?,
                method,
                Some(Value::Object(parameters)),
            ));
            let mut headers = HeaderMap::new();
            headers.insert(
                "mcp-protocol-version",
                HeaderValue::from_static(agentmesh_protocol::LATEST_PROTOCOL_VERSION),
            );
            let mut request =
                ProxyRequest::new(self.endpoint.clone(), message).with_headers(headers);
            if let Some(credential) = &self.credential {
                request = request.with_credential(credential.clone());
            }

            let response = self.proxy.execute(request).await?;
            let ProxyBody::Json(JsonRpcMessage::Response(response)) = response.into_parts().2
            else {
                return Err(protocol_error(
                    "The MCP discovery endpoint returned a non-JSON response.",
                ));
            };
            match response {
                JsonRpcResponse::Success(success) => Ok(success.result),
                JsonRpcResponse::Error(_) => Err(protocol_error(
                    "The MCP server rejected a capability discovery request.",
                )),
            }
        })
    }
}

fn encoding_error(source: serde_json::Error) -> AgentMeshError {
    AgentMeshError::with_source(
        ErrorCode::Internal,
        "The MCP discovery request could not be encoded.",
        source,
    )
}

pub(crate) fn protocol_error(message: &'static str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::UpstreamProtocolError, message)
}
