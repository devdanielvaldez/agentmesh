//! Endpoint provider contract and static bootstrap provider.

use std::{future::Future, pin::Pin};

use agentmesh_core::Transport;
use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_registry::RegistryScope;
use serde::{Deserialize, Serialize};

/// Endpoint candidate emitted by discovery before trust and health decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointCandidate {
    /// Transport used to reach the candidate.
    pub transport: Transport,
    /// Provider-specific HTTP URL or stdio target.
    pub target: String,
    /// Provider metadata available to later policy and reconciliation stages.
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,
}

impl EndpointCandidate {
    fn validate(&self) -> Result<(), AgentMeshError> {
        if self.target.trim().is_empty() || self.target.len() > 4_096 {
            return Err(AgentMeshError::new(
                ErrorCode::SchemaInvalid,
                "A discovered endpoint target is invalid.",
            ));
        }
        if self.labels.len() > 128
            || self
                .labels
                .iter()
                .any(|(key, value)| key.is_empty() || key.len() > 128 || value.len() > 1_024)
        {
            return Err(AgentMeshError::new(
                ErrorCode::SchemaInvalid,
                "Discovered endpoint labels exceed their configured bounds.",
            ));
        }
        Ok(())
    }
}

/// Future returned by object-safe endpoint providers.
pub type EndpointDiscoveryFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<EndpointCandidate>, AgentMeshError>> + Send + 'a>>;

/// Discovers endpoint candidates without deciding whether they are trusted.
pub trait EndpointProvider: Send + Sync {
    /// Returns candidates visible in one explicit tenant scope.
    fn discover(&self, scope: RegistryScope) -> EndpointDiscoveryFuture<'_>;
}

/// Deterministic bootstrap provider for local and file-backed configuration.
#[derive(Debug, Clone)]
pub struct StaticProvider {
    scope: RegistryScope,
    endpoints: Vec<EndpointCandidate>,
}

impl StaticProvider {
    /// Creates a bounded static provider.
    ///
    /// # Errors
    ///
    /// Returns a validation error for invalid candidates or more than 1,000 endpoints.
    pub fn new(
        scope: RegistryScope,
        endpoints: Vec<EndpointCandidate>,
    ) -> Result<Self, AgentMeshError> {
        if endpoints.len() > 1_000 {
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "Static endpoint discovery exceeds its candidate limit.",
            ));
        }
        for endpoint in &endpoints {
            endpoint.validate()?;
        }
        Ok(Self { scope, endpoints })
    }
}

impl EndpointProvider for StaticProvider {
    fn discover(&self, scope: RegistryScope) -> EndpointDiscoveryFuture<'_> {
        Box::pin(async move {
            if scope == self.scope {
                Ok(self.endpoints.clone())
            } else {
                Ok(Vec::new())
            }
        })
    }
}
