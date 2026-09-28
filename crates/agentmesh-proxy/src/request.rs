//! Owned proxy request and per-call overrides.

use std::{fmt, time::Duration};

use agentmesh_protocol::JsonRpcMessage;
use http::HeaderMap;

use crate::{UpstreamCredential, UpstreamEndpoint};

/// One validated MCP message to forward to a physical upstream endpoint.
pub struct ProxyRequest {
    pub(crate) endpoint: UpstreamEndpoint,
    pub(crate) message: JsonRpcMessage,
    pub(crate) headers: HeaderMap,
    pub(crate) credential: Option<UpstreamCredential>,
    pub(crate) timeout: Option<Duration>,
}

impl ProxyRequest {
    /// Creates a request without caller headers or upstream credentials.
    pub fn new(endpoint: UpstreamEndpoint, message: JsonRpcMessage) -> Self {
        Self {
            endpoint,
            message,
            headers: HeaderMap::new(),
            credential: None,
            timeout: None,
        }
    }

    /// Adds caller headers. The proxy applies its allowlist before forwarding.
    #[must_use]
    pub fn with_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }

    /// Adds a trusted upstream credential after caller headers are filtered.
    #[must_use]
    pub fn with_credential(mut self, credential: UpstreamCredential) -> Self {
        self.credential = Some(credential);
        self
    }

    /// Overrides the default end-to-end request timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

impl fmt::Debug for ProxyRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let header_names = self.headers.keys().collect::<Vec<_>>();
        formatter
            .debug_struct("ProxyRequest")
            .field("endpoint", &self.endpoint)
            .field("message", &"<redacted MCP message>")
            .field("header_names", &header_names)
            .field("has_credential", &self.credential.is_some())
            .field("timeout", &self.timeout)
            .finish()
    }
}
