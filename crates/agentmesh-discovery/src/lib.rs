//! Bounded discovery and safe reconciliation of MCP endpoints and capabilities.

mod catalog;
mod client;
mod endpoint;
mod engine;

pub use catalog::{CacheHint, CapabilityKey, DiscoveredCatalog, DiscoveryDiff, DiscoveryLimits};
pub use client::{DiscoveryClient, DiscoveryFuture, ProxyDiscoveryClient};
pub use endpoint::{
    ContainerProvider, DnsProvider, EndpointCandidate, EndpointDiscoveryFuture, EndpointProvider,
    KubernetesProvider, PluginCatalogProvider, SelfRegistrationProvider, StaticProvider,
};
pub use engine::{
    AcceptAllPolicy, ChangeAction, ConservativePolicy, DiscoveryDisposition, DiscoveryEngine,
    DiscoveryPolicy, DiscoveryResult,
};
