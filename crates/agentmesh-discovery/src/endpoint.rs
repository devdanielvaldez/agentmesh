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

/// DNS/SRV discovery adapter over trusted resolver output.
#[derive(Debug, Clone)]
pub struct DnsProvider(StaticProvider);
impl DnsProvider {
    /// Builds HTTP endpoints from bounded host/port records.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] when a generated candidate exceeds provider bounds.
    pub fn from_records(
        scope: RegistryScope,
        records: impl IntoIterator<Item = (String, u16)>,
        secure: bool,
    ) -> Result<Self, AgentMeshError> {
        let scheme = if secure { "https" } else { "http" };
        let endpoints = records
            .into_iter()
            .map(|(host, port)| EndpointCandidate {
                transport: Transport::StreamableHttp,
                target: format!("{scheme}://{host}:{port}/mcp"),
                labels: std::collections::BTreeMap::from([(
                    "discovery.provider".into(),
                    "dns".into(),
                )]),
            })
            .collect();
        StaticProvider::new(scope, endpoints).map(Self)
    }
}
impl EndpointProvider for DnsProvider {
    fn discover(&self, scope: RegistryScope) -> EndpointDiscoveryFuture<'_> {
        self.0.discover(scope)
    }
}

macro_rules! labeled_provider {
    ($name:ident, $label:literal, $description:literal) => {
        #[doc = $description]
        #[derive(Debug, Clone)]
        pub struct $name(StaticProvider);
        impl $name {
            /// Creates the provider from already authenticated platform metadata.
            ///
            /// # Errors
            ///
            /// Returns [`AgentMeshError`] when endpoint metadata exceeds provider bounds.
            pub fn new(
                scope: RegistryScope,
                mut endpoints: Vec<EndpointCandidate>,
            ) -> Result<Self, AgentMeshError> {
                for endpoint in &mut endpoints {
                    endpoint
                        .labels
                        .insert("discovery.provider".into(), $label.into());
                }
                StaticProvider::new(scope, endpoints).map(Self)
            }
        }
        impl EndpointProvider for $name {
            fn discover(&self, scope: RegistryScope) -> EndpointDiscoveryFuture<'_> {
                self.0.discover(scope)
            }
        }
    };
}

labeled_provider!(
    ContainerProvider,
    "container",
    "Container-label discovery provider."
);
labeled_provider!(
    KubernetesProvider,
    "kubernetes",
    "Kubernetes Service/Endpoint discovery provider."
);
labeled_provider!(
    PluginCatalogProvider,
    "plugin",
    "Plugin-supplied service catalog provider."
);

/// Authenticated self-registration provider with explicit workload identity binding.
#[derive(Debug, Clone)]
pub struct SelfRegistrationProvider {
    inner: StaticProvider,
    workload_identity: String,
}
impl SelfRegistrationProvider {
    /// Accepts endpoints only for a non-empty verified workload identity.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] for missing identity or invalid endpoint metadata.
    pub fn new(
        scope: RegistryScope,
        workload_identity: impl Into<String>,
        mut endpoints: Vec<EndpointCandidate>,
    ) -> Result<Self, AgentMeshError> {
        let workload_identity = workload_identity.into();
        if workload_identity.is_empty() || workload_identity.len() > 512 {
            return Err(AgentMeshError::new(
                ErrorCode::Unauthenticated,
                "A verified workload identity is required for self-registration.",
            ));
        }
        for endpoint in &mut endpoints {
            endpoint
                .labels
                .insert("discovery.provider".into(), "self-registration".into());
            endpoint
                .labels
                .insert("workload.identity".into(), workload_identity.clone());
        }
        Ok(Self {
            inner: StaticProvider::new(scope, endpoints)?,
            workload_identity,
        })
    }
    /// Verified workload identity associated with candidates.
    pub fn workload_identity(&self) -> &str {
        &self.workload_identity
    }
}
impl EndpointProvider for SelfRegistrationProvider {
    fn discover(&self, scope: RegistryScope) -> EndpointDiscoveryFuture<'_> {
        self.inner.discover(scope)
    }
}

#[cfg(test)]
mod provider_tests {
    use super::*;
    #[tokio::test]
    async fn provider_outputs_are_scoped_and_attributed() {
        let scope = RegistryScope::new("acme", "prod").unwrap();
        let provider =
            DnsProvider::from_records(scope.clone(), [("mcp.internal".into(), 443)], true).unwrap();
        let endpoints = provider.discover(scope).await.unwrap();
        assert_eq!(endpoints[0].labels["discovery.provider"], "dns");
        assert!(endpoints[0].target.starts_with("https://"));
    }
}
