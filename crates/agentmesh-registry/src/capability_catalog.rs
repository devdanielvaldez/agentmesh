//! Tenant-scoped, revisioned catalog of portable capability packages.

use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Mutex, RwLock},
};

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_protocol::CapabilityPackage;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::{RegistryRevision, RegistryScope};

/// Maximum number of capability versions stored in one tenant scope.
pub const MAX_CAPABILITY_PACKAGES_PER_SCOPE: usize = 10_000;
/// Maximum serialized size of one capability package.
pub const MAX_CAPABILITY_PACKAGE_BYTES: usize = 512 * 1_024;
/// Maximum encoded size of one tenant snapshot.
pub const MAX_CAPABILITY_SNAPSHOT_BYTES: usize = 64 * 1_024 * 1_024;

/// Optimistic write condition for a capability package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityWriteCondition {
    /// The capability ID and version must not already exist.
    Create,
    /// The stored package must have this catalog revision.
    Match(RegistryRevision),
}

/// One tenant-scoped published capability package.
#[derive(Debug, Clone, PartialEq)]
pub struct RegisteredCapability {
    /// Tenant owner.
    pub scope: RegistryScope,
    /// Portable package contract.
    pub package: CapabilityPackage,
    /// Catalog revision that last changed this record.
    pub revision: RegistryRevision,
}

/// Consistent immutable capability catalog snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct CapabilityCatalogSnapshot {
    /// Catalog revision at snapshot creation.
    pub revision: RegistryRevision,
    /// Stable ID/version ordering.
    pub capabilities: Vec<RegisteredCapability>,
}

/// Backend contract for tenant-scoped capability package distribution.
pub trait CapabilityCatalog: Send + Sync {
    /// Reads all packages in a tenant scope as one consistent snapshot.
    fn snapshot(&self, scope: &RegistryScope) -> Result<CapabilityCatalogSnapshot, AgentMeshError>;

    /// Reads one exact capability version.
    fn get(
        &self,
        scope: &RegistryScope,
        capability_id: &str,
        version: &str,
    ) -> Result<Option<RegisteredCapability>, AgentMeshError>;

    /// Creates or replaces a complete package atomically.
    fn publish(
        &self,
        scope: RegistryScope,
        package: CapabilityPackage,
        condition: CapabilityWriteCondition,
    ) -> Result<RegisteredCapability, AgentMeshError>;

    /// Revokes an exact version only when its revision matches.
    fn revoke(
        &self,
        scope: &RegistryScope,
        capability_id: &str,
        version: &str,
        expected: RegistryRevision,
    ) -> Result<RegistryRevision, AgentMeshError>;
}

#[derive(Default)]
struct State {
    revision: RegistryRevision,
    entries: BTreeMap<(RegistryScope, String, String), RegisteredCapability>,
}

/// Lock-protected in-memory catalog for tests and standalone deployments.
#[derive(Default)]
pub struct InMemoryCapabilityCatalog {
    state: RwLock<State>,
}

/// SQLite-backed tenant capability catalog for durable standalone deployments.
pub struct SqliteCapabilityCatalog {
    connection: Mutex<Connection>,
}

impl SqliteCapabilityCatalog {
    /// Opens or creates a persistent catalog database.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the database cannot be opened or migrated.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AgentMeshError> {
        let connection = Connection::open(path).map_err(|_| storage_error())?;
        Self::from_connection(connection)
    }

    /// Creates an isolated in-memory SQLite catalog.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the database schema cannot be initialized.
    pub fn in_memory() -> Result<Self, AgentMeshError> {
        let connection = Connection::open_in_memory().map_err(|_| storage_error())?;
        Self::from_connection(connection)
    }

    fn from_connection(connection: Connection) -> Result<Self, AgentMeshError> {
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(|_| storage_error())?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| storage_error())?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 CREATE TABLE IF NOT EXISTS capability_catalog_meta (
                    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                    revision INTEGER NOT NULL CHECK (revision >= 0)
                 );
                 INSERT OR IGNORE INTO capability_catalog_meta(singleton, revision) VALUES (1, 0);
                 CREATE TABLE IF NOT EXISTS capability_packages (
                    organization TEXT NOT NULL,
                    namespace TEXT NOT NULL,
                    capability_id TEXT NOT NULL,
                    capability_version TEXT NOT NULL,
                    revision INTEGER NOT NULL CHECK (revision > 0),
                    document BLOB NOT NULL,
                    PRIMARY KEY (organization, namespace, capability_id, capability_version)
                 );",
            )
            .map_err(|_| storage_error())?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }
}

impl InMemoryCapabilityCatalog {
    /// Creates an empty capability catalog.
    pub fn new() -> Self {
        Self::default()
    }
}

impl CapabilityCatalog for InMemoryCapabilityCatalog {
    fn snapshot(&self, scope: &RegistryScope) -> Result<CapabilityCatalogSnapshot, AgentMeshError> {
        let state = self.state.read().map_err(|_| storage_error())?;
        let mut total_bytes = 0_usize;
        let capabilities: Vec<_> = state
            .entries
            .values()
            .filter(|entry| &entry.scope == scope)
            .map(|entry| {
                let bytes = serde_json::to_vec(&entry.package)
                    .map_err(|_| invalid_package())?
                    .len();
                total_bytes = total_bytes.saturating_add(bytes);
                if total_bytes > MAX_CAPABILITY_SNAPSHOT_BYTES {
                    return Err(AgentMeshError::new(
                        ErrorCode::PayloadTooLarge,
                        "The capability catalog snapshot exceeds its size limit.",
                    ));
                }
                Ok(entry.clone())
            })
            .collect::<Result<_, AgentMeshError>>()?;
        Ok(CapabilityCatalogSnapshot {
            revision: state.revision,
            capabilities,
        })
    }

    fn get(
        &self,
        scope: &RegistryScope,
        capability_id: &str,
        version: &str,
    ) -> Result<Option<RegisteredCapability>, AgentMeshError> {
        let state = self.state.read().map_err(|_| storage_error())?;
        Ok(state
            .entries
            .get(&(scope.clone(), capability_id.into(), version.into()))
            .cloned())
    }

    fn publish(
        &self,
        scope: RegistryScope,
        package: CapabilityPackage,
        condition: CapabilityWriteCondition,
    ) -> Result<RegisteredCapability, AgentMeshError> {
        package.validate().map_err(|_| invalid_package())?;
        if package.provenance.is_some() && !package.verify_content_digest() {
            return Err(invalid_package());
        }
        let serialized_size = serde_json::to_vec(&package)
            .map_err(|_| invalid_package())?
            .len();
        if serialized_size > MAX_CAPABILITY_PACKAGE_BYTES {
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "The capability package exceeds its catalog size limit.",
            ));
        }
        let key = (
            scope.clone(),
            package.capability.id.clone(),
            package.capability.version.clone(),
        );
        let mut state = self.state.write().map_err(|_| storage_error())?;
        let current = state.entries.get(&key);
        match (current, condition) {
            (None, CapabilityWriteCondition::Create) => {}
            (Some(_), CapabilityWriteCondition::Create) => {
                return Err(conflict("The capability version already exists."));
            }
            (Some(current), CapabilityWriteCondition::Match(expected))
                if current.revision == expected => {}
            (Some(_), CapabilityWriteCondition::Match(_)) => {
                return Err(conflict("The capability revision does not match."));
            }
            (None, CapabilityWriteCondition::Match(_)) => {
                return Err(AgentMeshError::new(
                    ErrorCode::ServerNotFound,
                    "The capability package does not exist.",
                ));
            }
        }
        if matches!(condition, CapabilityWriteCondition::Create)
            && state
                .entries
                .keys()
                .filter(|(entry_scope, _, _)| entry_scope == &scope)
                .count()
                >= MAX_CAPABILITY_PACKAGES_PER_SCOPE
        {
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "The tenant capability catalog reached its package limit.",
            ));
        }
        state.revision = RegistryRevision::new(
            state
                .revision
                .get()
                .checked_add(1)
                .ok_or_else(storage_error)?,
        );
        let entry = RegisteredCapability {
            scope,
            package,
            revision: state.revision,
        };
        state.entries.insert(key, entry.clone());
        Ok(entry)
    }

    fn revoke(
        &self,
        scope: &RegistryScope,
        capability_id: &str,
        version: &str,
        expected: RegistryRevision,
    ) -> Result<RegistryRevision, AgentMeshError> {
        let key = (scope.clone(), capability_id.into(), version.into());
        let mut state = self.state.write().map_err(|_| storage_error())?;
        let Some(entry) = state.entries.get(&key) else {
            return Err(AgentMeshError::new(
                ErrorCode::ServerNotFound,
                "The capability package does not exist.",
            ));
        };
        if entry.revision != expected {
            return Err(conflict("The capability revision does not match."));
        }
        let next = RegistryRevision::new(
            state
                .revision
                .get()
                .checked_add(1)
                .ok_or_else(storage_error)?,
        );
        state.entries.remove(&key);
        state.revision = next;
        Ok(next)
    }
}

impl CapabilityCatalog for SqliteCapabilityCatalog {
    fn snapshot(&self, scope: &RegistryScope) -> Result<CapabilityCatalogSnapshot, AgentMeshError> {
        let connection = self.connection.lock().map_err(|_| storage_error())?;
        let revision = read_revision(&connection)?;
        let mut statement = connection
            .prepare(
                "SELECT revision, document FROM capability_packages
                 WHERE organization = ?1 AND namespace = ?2
                 ORDER BY capability_id, capability_version",
            )
            .map_err(|_| storage_error())?;
        let rows = statement
            .query_map(params![scope.organization(), scope.namespace()], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(|_| storage_error())?;
        let mut capabilities = Vec::new();
        let mut total_bytes = 0_usize;
        for row in rows {
            let (entry_revision, document) = row.map_err(|_| storage_error())?;
            total_bytes = total_bytes.saturating_add(document.len());
            if total_bytes > MAX_CAPABILITY_PACKAGES_PER_SCOPE * MAX_CAPABILITY_PACKAGE_BYTES {
                return Err(AgentMeshError::new(
                    ErrorCode::PayloadTooLarge,
                    "The capability catalog snapshot exceeds its size limit.",
                ));
            }
            let package: CapabilityPackage = decode_package(&document)?;
            capabilities.push(RegisteredCapability {
                scope: scope.clone(),
                package,
                revision: revision_from_sql(entry_revision)?,
            });
        }
        Ok(CapabilityCatalogSnapshot {
            revision,
            capabilities,
        })
    }

    fn get(
        &self,
        scope: &RegistryScope,
        capability_id: &str,
        version: &str,
    ) -> Result<Option<RegisteredCapability>, AgentMeshError> {
        let connection = self.connection.lock().map_err(|_| storage_error())?;
        let document = connection
            .query_row(
                "SELECT revision, document FROM capability_packages
                 WHERE organization = ?1 AND namespace = ?2
                   AND capability_id = ?3 AND capability_version = ?4",
                params![
                    scope.organization(),
                    scope.namespace(),
                    capability_id,
                    version
                ],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()
            .map_err(|_| storage_error())?;
        document
            .map(|(revision, document)| {
                Ok(RegisteredCapability {
                    scope: scope.clone(),
                    package: decode_package(&document)?,
                    revision: revision_from_sql(revision)?,
                })
            })
            .transpose()
    }

    fn publish(
        &self,
        scope: RegistryScope,
        package: CapabilityPackage,
        condition: CapabilityWriteCondition,
    ) -> Result<RegisteredCapability, AgentMeshError> {
        validate_package(&package)?;
        let document = serde_json::to_vec(&package).map_err(|_| invalid_package())?;
        if document.len() > MAX_CAPABILITY_PACKAGE_BYTES {
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "The capability package exceeds its catalog size limit.",
            ));
        }
        let id = package.capability.id.clone();
        let version = package.capability.version.clone();
        let mut connection = self.connection.lock().map_err(|_| storage_error())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| storage_error())?;
        let current: Option<i64> = transaction
            .query_row(
                "SELECT revision FROM capability_packages
                 WHERE organization = ?1 AND namespace = ?2
                   AND capability_id = ?3 AND capability_version = ?4",
                params![scope.organization(), scope.namespace(), id, version],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| storage_error())?;
        match (current, condition) {
            (None, CapabilityWriteCondition::Create) => {
                let count: i64 = transaction
                    .query_row(
                        "SELECT COUNT(*) FROM capability_packages WHERE organization = ?1 AND namespace = ?2",
                        params![scope.organization(), scope.namespace()],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(|_| storage_error())?;
                if usize::try_from(count).map_err(|_| storage_error())?
                    >= MAX_CAPABILITY_PACKAGES_PER_SCOPE
                {
                    return Err(AgentMeshError::new(
                        ErrorCode::PayloadTooLarge,
                        "The tenant capability catalog reached its package limit.",
                    ));
                }
            }
            (Some(_), CapabilityWriteCondition::Create) => {
                return Err(conflict("The capability version already exists."));
            }
            (Some(revision), CapabilityWriteCondition::Match(expected))
                if revision_from_sql(revision)? == expected => {}
            (Some(_), CapabilityWriteCondition::Match(_)) => {
                return Err(conflict("The capability revision does not match."));
            }
            (None, CapabilityWriteCondition::Match(_)) => {
                return Err(AgentMeshError::new(
                    ErrorCode::ServerNotFound,
                    "The capability package does not exist.",
                ));
            }
        }
        let revision = next_revision(&transaction)?;
        let revision_sql = revision_to_sql(revision)?;
        transaction
            .execute(
                "INSERT INTO capability_packages(
                    organization, namespace, capability_id, capability_version, revision, document
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(organization, namespace, capability_id, capability_version)
                 DO UPDATE SET revision = excluded.revision, document = excluded.document",
                params![
                    scope.organization(),
                    scope.namespace(),
                    id,
                    version,
                    revision_sql,
                    document
                ],
            )
            .map_err(|_| storage_error())?;
        transaction
            .execute(
                "UPDATE capability_catalog_meta SET revision = ?1 WHERE singleton = 1",
                [revision_sql],
            )
            .map_err(|_| storage_error())?;
        transaction.commit().map_err(|_| storage_error())?;
        Ok(RegisteredCapability {
            scope,
            package,
            revision,
        })
    }

    fn revoke(
        &self,
        scope: &RegistryScope,
        capability_id: &str,
        version: &str,
        expected: RegistryRevision,
    ) -> Result<RegistryRevision, AgentMeshError> {
        let mut connection = self.connection.lock().map_err(|_| storage_error())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| storage_error())?;
        let current: Option<i64> = transaction
            .query_row(
                "SELECT revision FROM capability_packages
                 WHERE organization = ?1 AND namespace = ?2
                   AND capability_id = ?3 AND capability_version = ?4",
                params![
                    scope.organization(),
                    scope.namespace(),
                    capability_id,
                    version
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| storage_error())?;
        match current {
            None => {
                return Err(AgentMeshError::new(
                    ErrorCode::ServerNotFound,
                    "The capability package does not exist.",
                ));
            }
            Some(revision) if revision_from_sql(revision)? != expected => {
                return Err(conflict("The capability revision does not match."));
            }
            Some(_) => {}
        }
        let revision = next_revision(&transaction)?;
        let revision_sql = revision_to_sql(revision)?;
        transaction
            .execute(
                "DELETE FROM capability_packages WHERE organization = ?1 AND namespace = ?2
                 AND capability_id = ?3 AND capability_version = ?4",
                params![
                    scope.organization(),
                    scope.namespace(),
                    capability_id,
                    version
                ],
            )
            .map_err(|_| storage_error())?;
        transaction
            .execute(
                "UPDATE capability_catalog_meta SET revision = ?1 WHERE singleton = 1",
                [revision_sql],
            )
            .map_err(|_| storage_error())?;
        transaction.commit().map_err(|_| storage_error())?;
        Ok(revision)
    }
}

fn validate_package(package: &CapabilityPackage) -> Result<(), AgentMeshError> {
    package.validate().map_err(|_| invalid_package())?;
    if package.provenance.is_some() && !package.verify_content_digest() {
        return Err(invalid_package());
    }
    Ok(())
}

fn decode_package(document: &[u8]) -> Result<CapabilityPackage, AgentMeshError> {
    let package: CapabilityPackage =
        serde_json::from_slice(document).map_err(|_| storage_error())?;
    validate_package(&package).map_err(|_| storage_error())?;
    Ok(package)
}

fn read_revision(connection: &Connection) -> Result<RegistryRevision, AgentMeshError> {
    let revision = connection
        .query_row(
            "SELECT revision FROM capability_catalog_meta WHERE singleton = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|_| storage_error())?;
    revision_from_sql(revision)
}

fn next_revision(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<RegistryRevision, AgentMeshError> {
    let current: i64 = transaction
        .query_row(
            "SELECT revision FROM capability_catalog_meta WHERE singleton = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|_| storage_error())?;
    revision_from_sql(current)?
        .next()
        .map_err(|_| storage_error())
}

fn revision_from_sql(value: i64) -> Result<RegistryRevision, AgentMeshError> {
    u64::try_from(value)
        .map(RegistryRevision::new)
        .map_err(|_| storage_error())
}

fn revision_to_sql(value: RegistryRevision) -> Result<i64, AgentMeshError> {
    i64::try_from(value.get()).map_err(|_| storage_error())
}

fn invalid_package() -> AgentMeshError {
    AgentMeshError::new(
        ErrorCode::SchemaInvalid,
        "The capability package is invalid or has a mismatched digest.",
    )
}

fn conflict(message: &'static str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::Conflict, message)
}

fn storage_error() -> AgentMeshError {
    AgentMeshError::new(
        ErrorCode::StorageUnavailable,
        "The capability catalog is unavailable.",
    )
}
