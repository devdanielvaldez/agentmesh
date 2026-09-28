//! Domain types shared by every `AgentMesh` component.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable identifier for a registered MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ServerId(Uuid);

impl ServerId {
    /// Creates a new random server identifier.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ServerId {
    fn default() -> Self {
        Self::new()
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
