//! Tenant-scoped, versioned catalog of MCP services and capabilities.

mod memory;
mod model;
mod sqlite;
mod store;

pub use memory::InMemoryRegistry;
pub use model::{
    Capability, CapabilityKind, Endpoint, EndpointId, EndpointLifecycle, RegisteredService,
    RegistryRevision, RegistryScope, RegistrySnapshot, ServiceLifecycle,
};
pub use sqlite::SqliteRegistry;
pub use store::{Registry, WriteCondition};
