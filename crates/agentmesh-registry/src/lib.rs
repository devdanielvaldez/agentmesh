//! Tenant-scoped, versioned catalog of MCP services and capabilities.

mod capability_catalog;
mod memory;
mod model;
mod sqlite;
mod store;

pub use capability_catalog::{
    CapabilityCatalog, CapabilityCatalogSnapshot, CapabilityWriteCondition,
    InMemoryCapabilityCatalog, MAX_CAPABILITY_PACKAGE_BYTES, MAX_CAPABILITY_PACKAGES_PER_SCOPE,
    MAX_CAPABILITY_SNAPSHOT_BYTES, RegisteredCapability, SqliteCapabilityCatalog,
};
pub use memory::InMemoryRegistry;
pub use model::{
    Capability, CapabilityKind, Endpoint, EndpointId, EndpointLifecycle, RegisteredService,
    RegistryRevision, RegistryScope, RegistrySnapshot, ServiceLifecycle,
};
pub use sqlite::SqliteRegistry;
pub use store::{Registry, WriteCondition};
