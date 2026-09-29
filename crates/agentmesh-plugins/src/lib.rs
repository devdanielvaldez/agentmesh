//! Compile-time plugin contracts with explicit permissions and bounded execution.

use agentmesh_error::{AgentMeshError, ErrorCode};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
    sync::{Arc, RwLock},
    time::Duration,
};

/// Extension point implemented by a plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginKind {
    /// Caller authentication verifier.
    Authentication,
    /// Endpoint discovery source.
    Discovery,
    /// Policy function.
    Policy,
    /// Secret material provider.
    SecretProvider,
    /// Telemetry or audit exporter.
    TelemetryExporter,
    /// Endpoint selection strategy.
    LoadBalancer,
    /// Request or response filter.
    Filter,
}

/// Capability granted by the operator, never inferred from plugin code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Outbound network access.
    Network,
    /// Brokered secret access.
    Secrets,
    /// Configuration snapshot access.
    Configuration,
    /// Telemetry emission.
    Telemetry,
}

/// Immutable plugin identity and requested privileges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDescriptor {
    /// Unique DNS-like identifier.
    pub name: String,
    /// Semantic implementation version.
    pub version: String,
    /// Extension point.
    pub kind: PluginKind,
    /// Permissions required at runtime.
    pub permissions: BTreeSet<Permission>,
}

impl PluginDescriptor {
    /// Validates bounded metadata.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] for empty or oversized fields.
    pub fn validate(&self) -> Result<(), AgentMeshError> {
        if self.name.is_empty()
            || self.name.len() > 128
            || self.version.is_empty()
            || self.version.len() > 64
        {
            Err(invalid("Plugin metadata is invalid."))
        } else {
            Ok(())
        }
    }
}

/// Per-call limits enforced by the host.
#[derive(Debug, Clone, Copy)]
pub struct PluginLimits {
    /// Maximum input/output bytes.
    pub max_payload_bytes: usize,
    /// Maximum call duration.
    pub timeout: Duration,
}
impl Default for PluginLimits {
    fn default() -> Self {
        Self {
            max_payload_bytes: 256 * 1024,
            timeout: Duration::from_secs(5),
        }
    }
}

/// Future returned by object-safe plugin calls.
pub type PluginFuture<'a> =
    Pin<Box<dyn Future<Output = Result<serde_json::Value, AgentMeshError>> + Send + 'a>>;

/// Language-neutral invocation boundary suitable for a future WASM adapter.
pub trait Plugin: Send + Sync {
    /// Declares identity, extension point, and requested permissions.
    fn descriptor(&self) -> &PluginDescriptor;
    /// Executes a bounded, tenant-explicit request.
    fn call<'a>(&'a self, context: &'a PluginContext, input: serde_json::Value)
    -> PluginFuture<'a>;
}

/// Non-secret execution context supplied by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginContext {
    /// Tenant boundary.
    pub tenant: String,
    /// Trace correlation identifier.
    pub trace_id: String,
    /// Permissions granted for this invocation.
    pub grants: BTreeSet<Permission>,
}

/// Registry enforcing uniqueness, allowlists, and payload bounds.
pub struct PluginRegistry {
    plugins: RwLock<BTreeMap<String, Arc<dyn Plugin>>>,
    allowed: BTreeSet<Permission>,
    limits: PluginLimits,
}
impl PluginRegistry {
    /// Creates a registry with an operator permission allowlist.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] when execution limits are zero.
    pub fn new(
        allowed: BTreeSet<Permission>,
        limits: PluginLimits,
    ) -> Result<Self, AgentMeshError> {
        if limits.max_payload_bytes == 0 || limits.timeout.is_zero() {
            return Err(invalid("Plugin limits must be positive."));
        }
        Ok(Self {
            plugins: RwLock::new(BTreeMap::new()),
            allowed,
            limits,
        })
    }
    /// Registers a plugin only when every requested permission is allowed.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] for invalid metadata, duplicate names, or denied permissions.
    pub fn register(&self, plugin: Arc<dyn Plugin>) -> Result<(), AgentMeshError> {
        let descriptor = plugin.descriptor();
        descriptor.validate()?;
        if !descriptor.permissions.is_subset(&self.allowed) {
            return Err(AgentMeshError::new(
                ErrorCode::PermissionDenied,
                "The plugin requests permissions that are not allowed.",
            ));
        }
        let mut plugins = self.plugins.write().map_err(|_| unavailable())?;
        if plugins.insert(descriptor.name.clone(), plugin).is_some() {
            return Err(AgentMeshError::new(
                ErrorCode::Conflict,
                "A plugin with this name is already registered.",
            ));
        }
        Ok(())
    }
    /// Invokes a plugin after checking tenant context, grants, and input/output sizes.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] for missing plugins, denied permissions, or execution failures.
    pub async fn invoke(
        &self,
        name: &str,
        context: &PluginContext,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, AgentMeshError> {
        if context.tenant.is_empty() || context.trace_id.is_empty() {
            return Err(invalid(
                "Plugin context must include tenant and trace identifiers.",
            ));
        }
        let plugin = self
            .plugins
            .read()
            .map_err(|_| unavailable())?
            .get(name)
            .cloned()
            .ok_or_else(|| {
                AgentMeshError::new(
                    ErrorCode::CapabilityNotFound,
                    "The requested plugin is not registered.",
                )
            })?;
        if !plugin.descriptor().permissions.is_subset(&context.grants) {
            return Err(AgentMeshError::new(
                ErrorCode::PermissionDenied,
                "The invocation does not grant required plugin permissions.",
            ));
        }
        check_size(&input, self.limits.max_payload_bytes)?;
        let output = tokio::time::timeout(self.limits.timeout, plugin.call(context, input))
            .await
            .map_err(|_| {
                AgentMeshError::new(
                    ErrorCode::UpstreamTimeout,
                    "The plugin exceeded its execution deadline.",
                )
            })??;
        check_size(&output, self.limits.max_payload_bytes)?;
        Ok(output)
    }
    /// Lists descriptors without exposing plugin internals.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] if the registry lock is unavailable.
    pub fn descriptors(&self) -> Result<Vec<PluginDescriptor>, AgentMeshError> {
        Ok(self
            .plugins
            .read()
            .map_err(|_| unavailable())?
            .values()
            .map(|plugin| plugin.descriptor().clone())
            .collect())
    }
}

fn check_size(value: &serde_json::Value, limit: usize) -> Result<(), AgentMeshError> {
    if serde_json::to_vec(value)
        .map_err(AgentMeshError::internal)?
        .len()
        > limit
    {
        Err(AgentMeshError::new(
            ErrorCode::PayloadTooLarge,
            "The plugin payload exceeds its configured bound.",
        ))
    } else {
        Ok(())
    }
}
fn invalid(message: &str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::ConfigurationInvalid, message)
}
fn unavailable() -> AgentMeshError {
    AgentMeshError::new(
        ErrorCode::ConfigurationUnavailable,
        "The plugin registry is unavailable.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Echo {
        descriptor: PluginDescriptor,
    }
    impl Plugin for Echo {
        fn descriptor(&self) -> &PluginDescriptor {
            &self.descriptor
        }
        fn call<'a>(&'a self, _: &'a PluginContext, input: serde_json::Value) -> PluginFuture<'a> {
            Box::pin(async move { Ok(input) })
        }
    }
    #[test]
    fn rejects_excess_permissions() {
        let registry = PluginRegistry::new(BTreeSet::new(), PluginLimits::default()).unwrap();
        let plugin = Arc::new(Echo {
            descriptor: PluginDescriptor {
                name: "example.echo".into(),
                version: "1.0.0".into(),
                kind: PluginKind::Filter,
                permissions: BTreeSet::from([Permission::Network]),
            },
        });
        assert!(registry.register(plugin).is_err());
    }
}
