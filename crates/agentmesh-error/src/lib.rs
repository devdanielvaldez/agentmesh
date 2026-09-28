//! Stable error taxonomy and disclosure-safe responses for `AgentMesh`.
//!
//! [`AgentMeshError`] deliberately separates its public message from its internal
//! source. Transports can serialize [`PublicErrorResponse`] without leaking the
//! source error, credentials, network locations, or storage implementation details.

use std::{error::Error, fmt};

use serde::Serialize;

/// Boxed internal error source accepted by [`AgentMeshError::internal`].
pub type BoxError = Box<dyn Error + Send + Sync + 'static>;

/// Stable machine-readable error code.
///
/// Variant names and serialized values form part of the public API. Existing
/// values must not be renamed or reused for a different meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum ErrorCode {
    /// The request envelope is invalid.
    InvalidRequest,
    /// The request body cannot be parsed as an MCP message.
    InvalidMessage,
    /// The requested MCP protocol version is not supported.
    UnsupportedProtocolVersion,
    /// The requested MCP method is not implemented.
    MethodNotSupported,
    /// A request or response exceeded its configured size limit.
    PayloadTooLarge,
    /// A tool or message schema is invalid.
    SchemaInvalid,
    /// The caller did not provide an acceptable identity.
    Unauthenticated,
    /// Supplied credentials are invalid.
    InvalidCredentials,
    /// Supplied credentials have expired.
    CredentialsExpired,
    /// The principal is not authorized for the action.
    PermissionDenied,
    /// A policy explicitly denied the action.
    PolicyDenied,
    /// The action cannot continue without human approval.
    ApprovalRequired,
    /// The requested logical MCP server does not exist.
    ServerNotFound,
    /// The requested tool, resource, prompt, or task handler does not exist.
    CapabilityNotFound,
    /// No route matches the normalized request.
    RouteNotFound,
    /// No endpoint is currently eligible to receive the request.
    NoHealthyEndpoints,
    /// `AgentMesh` could not connect to the selected upstream.
    UpstreamConnectionFailed,
    /// The upstream did not complete before its deadline.
    UpstreamTimeout,
    /// The upstream returned an invalid MCP response.
    UpstreamProtocolError,
    /// The upstream is temporarily unavailable.
    UpstreamUnavailable,
    /// The endpoint circuit breaker is open.
    CircuitOpen,
    /// A request-rate limit was exceeded.
    RateLimited,
    /// A concurrent-request limit was exceeded.
    ConcurrencyLimited,
    /// A usage quota was exhausted.
    QuotaExceeded,
    /// Configuration failed schema or semantic validation.
    ConfigurationInvalid,
    /// Valid runtime configuration is temporarily unavailable.
    ConfigurationUnavailable,
    /// A required persistence backend is unavailable.
    StorageUnavailable,
    /// The operation conflicts with the current resource revision or state.
    Conflict,
    /// No MCP upstream has been configured for the gateway.
    McpProxyNotConfigured,
    /// An unexpected internal failure occurred.
    Internal,
}

impl ErrorCode {
    /// Every currently defined code, useful for documentation and conformance tests.
    pub const ALL: [Self; 30] = [
        Self::InvalidRequest,
        Self::InvalidMessage,
        Self::UnsupportedProtocolVersion,
        Self::MethodNotSupported,
        Self::PayloadTooLarge,
        Self::SchemaInvalid,
        Self::Unauthenticated,
        Self::InvalidCredentials,
        Self::CredentialsExpired,
        Self::PermissionDenied,
        Self::PolicyDenied,
        Self::ApprovalRequired,
        Self::ServerNotFound,
        Self::CapabilityNotFound,
        Self::RouteNotFound,
        Self::NoHealthyEndpoints,
        Self::UpstreamConnectionFailed,
        Self::UpstreamTimeout,
        Self::UpstreamProtocolError,
        Self::UpstreamUnavailable,
        Self::CircuitOpen,
        Self::RateLimited,
        Self::ConcurrencyLimited,
        Self::QuotaExceeded,
        Self::ConfigurationInvalid,
        Self::ConfigurationUnavailable,
        Self::StorageUnavailable,
        Self::Conflict,
        Self::McpProxyNotConfigured,
        Self::Internal,
    ];

    /// Returns the broad operational category for metrics and policy.
    pub const fn category(self) -> ErrorCategory {
        match self {
            Self::InvalidRequest
            | Self::InvalidMessage
            | Self::UnsupportedProtocolVersion
            | Self::MethodNotSupported
            | Self::PayloadTooLarge
            | Self::SchemaInvalid => ErrorCategory::Protocol,
            Self::Unauthenticated | Self::InvalidCredentials | Self::CredentialsExpired => {
                ErrorCategory::Authentication
            }
            Self::PermissionDenied => ErrorCategory::Authorization,
            Self::PolicyDenied | Self::ApprovalRequired => ErrorCategory::Policy,
            Self::ServerNotFound | Self::CapabilityNotFound => ErrorCategory::Discovery,
            Self::RouteNotFound | Self::NoHealthyEndpoints => ErrorCategory::Routing,
            Self::UpstreamConnectionFailed
            | Self::UpstreamTimeout
            | Self::UpstreamProtocolError
            | Self::UpstreamUnavailable
            | Self::CircuitOpen
            | Self::McpProxyNotConfigured => ErrorCategory::Upstream,
            Self::RateLimited | Self::ConcurrencyLimited | Self::QuotaExceeded => {
                ErrorCategory::Limit
            }
            Self::ConfigurationInvalid | Self::ConfigurationUnavailable => {
                ErrorCategory::Configuration
            }
            Self::StorageUnavailable => ErrorCategory::Storage,
            Self::Conflict | Self::Internal => ErrorCategory::Internal,
        }
    }

    /// Returns the recommended HTTP status code without coupling this crate to an HTTP library.
    pub const fn http_status(self) -> u16 {
        match self {
            Self::InvalidRequest
            | Self::InvalidMessage
            | Self::UnsupportedProtocolVersion
            | Self::SchemaInvalid
            | Self::ConfigurationInvalid => 400,
            Self::Unauthenticated | Self::InvalidCredentials | Self::CredentialsExpired => 401,
            Self::PermissionDenied | Self::PolicyDenied | Self::ApprovalRequired => 403,
            Self::ServerNotFound | Self::CapabilityNotFound | Self::RouteNotFound => 404,
            Self::Conflict => 409,
            Self::PayloadTooLarge => 413,
            Self::RateLimited | Self::ConcurrencyLimited | Self::QuotaExceeded => 429,
            Self::MethodNotSupported => 501,
            Self::UpstreamTimeout => 504,
            Self::NoHealthyEndpoints
            | Self::UpstreamConnectionFailed
            | Self::UpstreamProtocolError
            | Self::UpstreamUnavailable
            | Self::CircuitOpen
            | Self::ConfigurationUnavailable
            | Self::StorageUnavailable
            | Self::McpProxyNotConfigured => 503,
            Self::Internal => 500,
        }
    }

    /// Returns the JSON-RPC/MCP error number recommended for the code.
    pub const fn mcp_code(self) -> i32 {
        match self {
            Self::InvalidMessage => -32700,
            Self::InvalidRequest => -32600,
            Self::MethodNotSupported => -32601,
            Self::SchemaInvalid => -32602,
            Self::Internal => -32603,
            Self::Unauthenticated | Self::InvalidCredentials | Self::CredentialsExpired => -32001,
            Self::PermissionDenied | Self::PolicyDenied | Self::ApprovalRequired => -32003,
            Self::ServerNotFound | Self::CapabilityNotFound | Self::RouteNotFound => -32004,
            Self::RateLimited | Self::ConcurrencyLimited | Self::QuotaExceeded => -32029,
            Self::UpstreamTimeout => -32040,
            Self::UnsupportedProtocolVersion
            | Self::PayloadTooLarge
            | Self::NoHealthyEndpoints
            | Self::UpstreamConnectionFailed
            | Self::UpstreamProtocolError
            | Self::UpstreamUnavailable
            | Self::CircuitOpen
            | Self::ConfigurationInvalid
            | Self::ConfigurationUnavailable
            | Self::StorageUnavailable
            | Self::Conflict
            | Self::McpProxyNotConfigured => -32000,
        }
    }

    /// Classifies whether an automated retry can be considered.
    pub const fn retry_class(self) -> RetryClass {
        match self {
            Self::NoHealthyEndpoints
            | Self::UpstreamConnectionFailed
            | Self::UpstreamTimeout
            | Self::UpstreamUnavailable
            | Self::CircuitOpen
            | Self::RateLimited
            | Self::ConcurrencyLimited
            | Self::ConfigurationUnavailable
            | Self::StorageUnavailable => RetryClass::Backoff,
            _ => RetryClass::Never,
        }
    }
}

/// Broad family used for low-cardinality metrics and operational policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorCategory {
    /// MCP or message validation failure.
    Protocol,
    /// Caller authentication failure.
    Authentication,
    /// Principal authorization failure.
    Authorization,
    /// Contextual policy outcome.
    Policy,
    /// Registry or capability lookup failure.
    Discovery,
    /// Route resolution failure.
    Routing,
    /// Upstream availability or protocol failure.
    Upstream,
    /// Rate, concurrency, or quota limit.
    Limit,
    /// Configuration loading or validation failure.
    Configuration,
    /// Persistence backend failure.
    Storage,
    /// Unexpected or state-conflict failure.
    Internal,
}

/// Guidance for retry orchestration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryClass {
    /// The same request must not be retried automatically.
    Never,
    /// A retry may be attempted after policy-controlled backoff.
    Backoff,
}

/// Internal platform error with a disclosure-safe public representation.
pub struct AgentMeshError {
    code: ErrorCode,
    public_message: String,
    trace_id: Option<String>,
    source: Option<BoxError>,
}

impl AgentMeshError {
    /// Creates an error whose message is safe to return to an untrusted client.
    pub fn new(code: ErrorCode, public_message: impl Into<String>) -> Self {
        Self {
            code,
            public_message: public_message.into(),
            trace_id: None,
            source: None,
        }
    }

    /// Wraps an internal cause while exposing only a generic client message.
    pub fn internal(source: impl Into<BoxError>) -> Self {
        Self {
            code: ErrorCode::Internal,
            public_message: "An unexpected internal error occurred.".into(),
            trace_id: None,
            source: Some(source.into()),
        }
    }

    /// Associates a trace identifier that clients may safely use when requesting support.
    #[must_use]
    pub fn with_trace_id(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }

    /// Returns the stable machine-readable code.
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    /// Returns the disclosure-safe message.
    pub fn public_message(&self) -> &str {
        &self.public_message
    }

    /// Returns the optional trace identifier.
    pub fn trace_id(&self) -> Option<&str> {
        self.trace_id.as_deref()
    }

    /// Creates an owned response containing only fields approved for client disclosure.
    pub fn public_response(&self) -> PublicErrorResponse {
        PublicErrorResponse {
            error: PublicError {
                code: self.code,
                message: self.public_message.clone(),
                retryable: self.code.retry_class() != RetryClass::Never,
                trace_id: self.trace_id.clone(),
            },
        }
    }
}

impl fmt::Display for AgentMeshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.public_message)
    }
}

impl fmt::Debug for AgentMeshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentMeshError")
            .field("code", &self.code)
            .field("public_message", &self.public_message)
            .field("trace_id", &self.trace_id)
            .field("has_source", &self.source.is_some())
            .finish()
    }
}

impl Error for AgentMeshError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_deref().map(|source| source as _)
    }
}

/// JSON-safe error envelope returned by HTTP and administrative APIs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicErrorResponse {
    /// Public error body.
    pub error: PublicError,
}

/// Fields approved for disclosure to an untrusted client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicError {
    /// Stable machine-readable code.
    pub code: ErrorCode,
    /// Human-readable message without internal details.
    pub message: String,
    /// Whether retry orchestration may consider the failure transient.
    pub retryable: bool,
    /// Correlation identifier suitable for support requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, error::Error as _};

    use super::*;

    #[test]
    fn every_code_has_unique_serialized_value() {
        let values: HashSet<_> = ErrorCode::ALL
            .iter()
            .map(|code| serde_json::to_string(code).expect("serialize code"))
            .collect();

        assert_eq!(values.len(), ErrorCode::ALL.len());
    }

    #[test]
    fn internal_source_is_not_disclosed() {
        let error = AgentMeshError::internal(std::io::Error::other(
            "database password secret-value was rejected",
        ))
        .with_trace_id("trace-123");

        let json = serde_json::to_string(&error.public_response()).expect("serialize response");
        let debug = format!("{error:?}");

        assert!(!json.contains("secret-value"));
        assert!(!debug.contains("secret-value"));
        assert!(error.source().is_some());
        assert!(json.contains("trace-123"));
    }

    #[test]
    fn mappings_are_stable_for_representative_codes() {
        assert_eq!(ErrorCode::InvalidRequest.http_status(), 400);
        assert_eq!(ErrorCode::InvalidRequest.mcp_code(), -32600);
        assert_eq!(ErrorCode::PermissionDenied.http_status(), 403);
        assert_eq!(ErrorCode::RateLimited.http_status(), 429);
        assert_eq!(ErrorCode::UpstreamTimeout.http_status(), 504);
        assert_eq!(ErrorCode::Internal.http_status(), 500);
    }

    #[test]
    fn only_transient_codes_are_retryable() {
        let transient = AgentMeshError::new(ErrorCode::UpstreamUnavailable, "Try again later.");
        let permanent = AgentMeshError::new(ErrorCode::PolicyDenied, "Request denied.");

        assert!(transient.public_response().error.retryable);
        assert!(!permanent.public_response().error.retryable);
    }
}
