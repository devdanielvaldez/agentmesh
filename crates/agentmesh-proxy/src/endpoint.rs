//! Validated upstream MCP endpoint.

use std::fmt;

use agentmesh_error::{AgentMeshError, ErrorCode};
use url::Url;

/// Parsed MCP upstream URL that passed baseline proxy checks.
#[derive(Clone)]
pub struct UpstreamEndpoint(Url);

impl UpstreamEndpoint {
    /// Parses an upstream URL.
    ///
    /// HTTPS is required unless `allow_insecure_http` is explicitly enabled for
    /// local development. Embedded credentials and fragments are always rejected.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] for malformed URLs, unsupported schemes,
    /// missing hosts, embedded credentials, or fragments.
    pub fn parse(value: &str, allow_insecure_http: bool) -> Result<Self, AgentMeshError> {
        let url = Url::parse(value).map_err(|_| invalid_endpoint())?;
        let permitted_scheme =
            url.scheme() == "https" || (allow_insecure_http && url.scheme() == "http");
        if !permitted_scheme
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(invalid_endpoint());
        }
        Ok(Self(url))
    }

    /// Returns the validated URL for the HTTP client.
    pub fn as_url(&self) -> &Url {
        &self.0
    }
}

impl fmt::Debug for UpstreamEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UpstreamEndpoint")
            .field("scheme", &self.0.scheme())
            .field("host", &self.0.host_str())
            .field("port", &self.0.port_or_known_default())
            .field("has_non_root_path", &(self.0.path() != "/"))
            .field("has_query", &self.0.query().is_some())
            .finish()
    }
}

fn invalid_endpoint() -> AgentMeshError {
    AgentMeshError::new(
        ErrorCode::ConfigurationInvalid,
        "The MCP upstream endpoint is invalid or insecure.",
    )
}
