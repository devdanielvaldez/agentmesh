//! Validated JSON responses and bounded SSE body streams.

use std::{fmt, pin::Pin};

use agentmesh_error::AgentMeshError;
use agentmesh_protocol::JsonRpcMessage;
use bytes::Bytes;
use futures_util::Stream;
use http::{HeaderMap, StatusCode};

/// Streaming response body returned for `text/event-stream`.
pub type ProxyStream = Pin<Box<dyn Stream<Item = Result<Bytes, AgentMeshError>> + Send + 'static>>;

/// Upstream response body after media-type validation.
pub enum ProxyBody {
    /// No body, normally for an accepted notification.
    Empty,
    /// One completely decoded JSON-RPC object.
    Json(JsonRpcMessage),
    /// Raw request-scoped SSE bytes with cumulative size enforcement.
    ServerSentEvents(ProxyStream),
}

impl fmt::Debug for ProxyBody {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("Empty"),
            Self::Json(message) => formatter.debug_tuple("Json").field(message).finish(),
            Self::ServerSentEvents(_) => formatter.write_str("ServerSentEvents(<stream>)"),
        }
    }
}

/// Safe upstream response ready for a gateway adapter.
#[derive(Debug)]
pub struct ProxyResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: ProxyBody,
}

impl ProxyResponse {
    pub(crate) const fn new(status: StatusCode, headers: HeaderMap, body: ProxyBody) -> Self {
        Self {
            status,
            headers,
            body,
        }
    }

    /// Returns the upstream HTTP status.
    pub const fn status(&self) -> StatusCode {
        self.status
    }

    /// Returns the filtered response headers.
    pub const fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Consumes the response into its status, headers, and body.
    pub fn into_parts(self) -> (StatusCode, HeaderMap, ProxyBody) {
        (self.status, self.headers, self.body)
    }
}
