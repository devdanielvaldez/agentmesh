//! Bounded discovery and safe reconciliation of MCP endpoints and capabilities.

mod catalog;
mod client;
mod endpoint;
mod engine;

pub use catalog::{CacheHint, CapabilityKey, DiscoveredCatalog, DiscoveryDiff, DiscoveryLimits};
pub use client::{DiscoveryClient, DiscoveryFuture, ProxyDiscoveryClient};
pub use endpoint::{EndpointCandidate, EndpointDiscoveryFuture, EndpointProvider, StaticProvider};
pub use engine::{
    AcceptAllPolicy, ChangeAction, ConservativePolicy, DiscoveryDisposition, DiscoveryEngine,
    DiscoveryPolicy, DiscoveryResult,
};
