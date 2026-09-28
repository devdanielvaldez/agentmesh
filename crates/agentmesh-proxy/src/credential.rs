//! Explicit upstream credentials isolated from caller headers.

use std::fmt;

use agentmesh_error::{AgentMeshError, ErrorCode};
use http::{HeaderName, HeaderValue, header};

/// Credential injected after untrusted caller headers have been filtered.
#[derive(Clone)]
pub struct UpstreamCredential {
    name: HeaderName,
    value: HeaderValue,
}

impl UpstreamCredential {
    /// Creates an arbitrary secret header credential.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] if the name or value is not a valid HTTP header.
    pub fn header(name: &str, value: &str) -> Result<Self, AgentMeshError> {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| invalid_credential())?;
        let value = HeaderValue::from_str(value).map_err(|_| invalid_credential())?;
        Ok(Self { name, value })
    }

    /// Creates an HTTP bearer credential.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] if the token cannot be represented safely in
    /// an HTTP header.
    pub fn bearer(token: &str) -> Result<Self, AgentMeshError> {
        Self::header(header::AUTHORIZATION.as_str(), &format!("Bearer {token}"))
    }

    pub(crate) fn parts(&self) -> (&HeaderName, &HeaderValue) {
        (&self.name, &self.value)
    }
}

impl fmt::Debug for UpstreamCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UpstreamCredential")
            .field("name", &self.name)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

fn invalid_credential() -> AgentMeshError {
    AgentMeshError::new(
        ErrorCode::ConfigurationInvalid,
        "The upstream credential header is invalid.",
    )
}
