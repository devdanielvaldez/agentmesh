//! Domain types shared by every `AgentMesh` component.

use std::{fmt, str::FromStr, time::Duration};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable identifier for a registered MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ServerId(Uuid);

impl ServerId {
    /// Creates a new random server identifier.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Creates an identifier from an existing UUID.
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    /// Returns the underlying UUID.
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for ServerId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ServerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for ServerId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

/// Operational state of an MCP endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// Endpoint accepts normal traffic.
    Healthy,
    /// Endpoint may accept reduced traffic.
    Degraded,
    /// Endpoint is excluded from routing.
    Unhealthy,
    /// Endpoint is finishing in-flight work only.
    Draining,
    /// Endpoint is administratively disabled.
    Disabled,
}

/// Transport used to reach an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    /// MCP Streamable HTTP transport.
    StreamableHttp,
    /// Locally supervised standard-I/O process.
    Stdio,
}

/// A registered MCP service and its replicas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServer {
    /// Stable server identifier.
    pub id: ServerId,
    /// Logical, human-readable service name.
    pub name: String,
    /// One or more physical endpoints.
    pub endpoints: Vec<String>,
    /// Configured transport.
    pub transport: Transport,
    /// Aggregated health state.
    pub health: HealthStatus,
}

/// Result of policy evaluation for a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecision {
    /// Permit the request.
    Allow,
    /// Reject the request.
    Deny,
    /// Pause until an authorized human decides.
    RequireApproval,
}

macro_rules! string_id {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);
        impl $name {
            /// Creates a bounded identifier containing safe ASCII characters.
            ///
            /// # Errors
            ///
            /// Returns [`IdentifierError`] for empty, oversized, or unsafe values.
            pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
                let value = value.into();
                if value.is_empty()
                    || value.len() > 128
                    || !value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
                {
                    return Err(IdentifierError);
                }
                Ok(Self(value))
            }
            /// Returns the string representation.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

/// A scoped identifier failed validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentifierError;
impl fmt::Display for IdentifierError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("identifier must contain 1-128 safe ASCII characters")
    }
}
impl std::error::Error for IdentifierError {}

string_id!(OrganizationId, "Stable organization identifier.");
string_id!(NamespaceId, "Stable namespace identifier.");
string_id!(PrincipalId, "Authenticated principal identifier.");
string_id!(AgentId, "AI agent identifier.");
string_id!(UserId, "Delegated human user identifier.");
string_id!(ServiceId, "Logical MCP service identifier.");
string_id!(CapabilityId, "Normalized capability identifier.");
string_id!(RequestId, "Platform request correlation identifier.");
string_id!(TraceId, "Distributed trace correlation identifier.");

/// Explicit tenant boundary used by domain operations.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TenantScope {
    /// Owning organization.
    pub organization: OrganizationId,
    /// Namespace within the organization.
    pub namespace: NamespaceId,
}

/// Monotonic desired-state revision.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ConfigRevision(pub u64);

/// Risk assigned to a capability or action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// Routine read-only behavior.
    Low,
    /// Material but reversible behavior.
    Medium,
    /// Sensitive or mutating behavior.
    High,
    /// Privileged behavior requiring strongest controls.
    Critical,
}

/// Immutable context passed explicitly through the data plane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestContext {
    /// Request correlation ID.
    pub request_id: RequestId,
    /// Trace correlation ID.
    pub trace_id: TraceId,
    /// Explicit tenant scope.
    pub scope: TenantScope,
    /// Authenticated actor.
    pub principal: PrincipalId,
    /// Optional delegated user.
    pub delegated_user: Option<UserId>,
    /// Protocol method.
    pub method: String,
    /// Optional capability.
    pub capability: Option<CapabilityId>,
    /// Configuration revision used for decisions.
    pub config_revision: ConfigRevision,
    /// End-to-end deadline from admission.
    pub deadline_ms: u64,
}

impl RequestContext {
    /// Returns the configured deadline as a duration.
    pub const fn deadline(&self) -> Duration {
        Duration::from_millis(self.deadline_ms)
    }
}

/// Injectable monotonic clock contract used by domain state machines.
pub trait Clock: Send + Sync {
    /// Returns milliseconds since an implementation-defined monotonic epoch.
    fn now_millis(&self) -> u64;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scoped_identifiers_are_validated() {
        assert!(OrganizationId::new("acme-prod").is_ok());
        assert!(OrganizationId::new("bad/value").is_err());
    }
}
