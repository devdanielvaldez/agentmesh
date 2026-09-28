//! Durable `SQLite` registry for local and single-node deployments.

use std::{path::Path, sync::Mutex, time::Duration};

use agentmesh_core::ServerId;
use agentmesh_error::{AgentMeshError, ErrorCode};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::{
    RegisteredService, Registry, RegistryRevision, RegistryScope, RegistrySnapshot, WriteCondition,
    memory::{conflict, not_found, verify_write_condition},
    model::{MAX_SERVICES_PER_SCOPE, MAX_SNAPSHOT_BYTES},
};

const SCHEMA_VERSION: i64 = 1;

/// SQLite-backed registry with transactional revisions and optimistic writes.
pub struct SqliteRegistry {
    connection: Mutex<Connection>,
}

impl SqliteRegistry {
    /// Opens or creates a registry database and applies idempotent migrations.
    ///
    /// # Errors
    ///
    /// Returns a storage error when `SQLite` cannot be opened or initialized.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AgentMeshError> {
        let connection = Connection::open(path).map_err(storage_error)?;
        Self::from_connection(connection)
    }

    /// Creates an isolated in-memory `SQLite` registry.
    ///
    /// # Errors
    ///
    /// Returns a storage error when `SQLite` initialization fails.
    pub fn in_memory() -> Result<Self, AgentMeshError> {
        let connection = Connection::open_in_memory().map_err(storage_error)?;
        Self::from_connection(connection)
    }

    fn from_connection(connection: Connection) -> Result<Self, AgentMeshError> {
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(storage_error)?;
        let schema_version = connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .map_err(storage_error)?;
        if schema_version > SCHEMA_VERSION {
            return Err(AgentMeshError::new(
                ErrorCode::StorageUnavailable,
                "The registry database schema is newer than this AgentMesh version.",
            ));
        }
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE IF NOT EXISTS registry_meta (
                     singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                     revision INTEGER NOT NULL CHECK (revision >= 0)
                 );
                 INSERT OR IGNORE INTO registry_meta(singleton, revision) VALUES (1, 0);
                 CREATE TABLE IF NOT EXISTS registry_services (
                     organization TEXT NOT NULL,
                     namespace TEXT NOT NULL,
                     server_id TEXT NOT NULL,
                     name TEXT NOT NULL,
                     revision INTEGER NOT NULL CHECK (revision > 0),
                     document TEXT NOT NULL,
                     PRIMARY KEY (organization, namespace, server_id),
                     UNIQUE (organization, namespace, name)
                 );
                 CREATE INDEX IF NOT EXISTS registry_services_scope_name
                 ON registry_services(organization, namespace, name, server_id);
                 PRAGMA user_version = 1;",
            )
            .map_err(storage_error)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }
}

impl Registry for SqliteRegistry {
    fn snapshot(&self, scope: &RegistryScope) -> Result<RegistrySnapshot, AgentMeshError> {
        let mut connection = self.connection.lock().map_err(|error| lock_error(&error))?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let revision = read_revision(&transaction)?;
        let limit = i64::try_from(MAX_SERVICES_PER_SCOPE + 1).map_err(numeric_error)?;
        let documents = {
            let mut statement = transaction
                .prepare(
                    "SELECT document FROM registry_services
                     WHERE organization = ?1 AND namespace = ?2
                     ORDER BY name, server_id
                     LIMIT ?3",
                )
                .map_err(storage_error)?;
            statement
                .query_map(
                    params![scope.organization(), scope.namespace(), limit],
                    |row| row.get::<_, String>(0),
                )
                .map_err(storage_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_error)?
        };
        if documents.len() > MAX_SERVICES_PER_SCOPE {
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "The registry snapshot exceeds its service limit.",
            ));
        }
        if documents
            .iter()
            .try_fold(0_usize, |size, document| size.checked_add(document.len()))
            .is_none_or(|size| size > MAX_SNAPSHOT_BYTES)
        {
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "The registry snapshot exceeds its size limit.",
            ));
        }
        let services = documents
            .into_iter()
            .map(|document| decode_service(&document))
            .collect::<Result<Vec<_>, _>>()?;
        transaction.commit().map_err(storage_error)?;
        Ok(RegistrySnapshot { revision, services })
    }

    fn get(
        &self,
        scope: &RegistryScope,
        id: ServerId,
    ) -> Result<Option<RegisteredService>, AgentMeshError> {
        let connection = self.connection.lock().map_err(|error| lock_error(&error))?;
        let document = connection
            .query_row(
                "SELECT document FROM registry_services
                 WHERE organization = ?1 AND namespace = ?2 AND server_id = ?3",
                params![scope.organization(), scope.namespace(), id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_error)?;
        document.map(|value| decode_service(&value)).transpose()
    }

    fn put(
        &self,
        mut service: RegisteredService,
        condition: WriteCondition,
    ) -> Result<RegisteredService, AgentMeshError> {
        service.validate()?;
        let mut connection = self.connection.lock().map_err(|error| lock_error(&error))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let current = read_service(&transaction, &service.scope, service.id)?;
        verify_write_condition(current.as_ref(), condition)?;
        verify_name_available(&transaction, &service)?;
        if matches!(condition, WriteCondition::Create)
            && count_services(&transaction, &service.scope)? >= MAX_SERVICES_PER_SCOPE
        {
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "The registry scope reached its service limit.",
            ));
        }

        let revision = advance_revision(&transaction)?;
        service.revision = revision;
        let document = serde_json::to_string(&service).map_err(storage_error)?;
        transaction
            .execute(
                "INSERT INTO registry_services
                    (organization, namespace, server_id, name, revision, document)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(organization, namespace, server_id) DO UPDATE SET
                    name = excluded.name,
                    revision = excluded.revision,
                    document = excluded.document",
                params![
                    service.scope.organization(),
                    service.scope.namespace(),
                    service.id.to_string(),
                    service.name,
                    revision_to_sql(revision)?,
                    document,
                ],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(service)
    }

    fn remove(
        &self,
        scope: &RegistryScope,
        id: ServerId,
        expected: RegistryRevision,
    ) -> Result<RegistryRevision, AgentMeshError> {
        let mut connection = self.connection.lock().map_err(|error| lock_error(&error))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let current = read_service(&transaction, scope, id)?.ok_or_else(not_found)?;
        if current.revision != expected {
            return Err(conflict("The service revision does not match."));
        }
        let revision = advance_revision(&transaction)?;
        transaction
            .execute(
                "DELETE FROM registry_services
                 WHERE organization = ?1 AND namespace = ?2 AND server_id = ?3",
                params![scope.organization(), scope.namespace(), id.to_string()],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(revision)
    }
}

fn read_service(
    transaction: &Transaction<'_>,
    scope: &RegistryScope,
    id: ServerId,
) -> Result<Option<RegisteredService>, AgentMeshError> {
    let document = transaction
        .query_row(
            "SELECT document FROM registry_services
             WHERE organization = ?1 AND namespace = ?2 AND server_id = ?3",
            params![scope.organization(), scope.namespace(), id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_error)?;
    document.map(|value| decode_service(&value)).transpose()
}

fn verify_name_available(
    transaction: &Transaction<'_>,
    service: &RegisteredService,
) -> Result<(), AgentMeshError> {
    let owner = transaction
        .query_row(
            "SELECT server_id FROM registry_services
             WHERE organization = ?1 AND namespace = ?2 AND name = ?3",
            params![
                service.scope.organization(),
                service.scope.namespace(),
                service.name
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_error)?;
    if owner.is_some_and(|owner| owner != service.id.to_string()) {
        return Err(conflict("A service with this name already exists."));
    }
    Ok(())
}

fn count_services(
    transaction: &Transaction<'_>,
    scope: &RegistryScope,
) -> Result<usize, AgentMeshError> {
    let count = transaction
        .query_row(
            "SELECT COUNT(*) FROM registry_services
             WHERE organization = ?1 AND namespace = ?2",
            params![scope.organization(), scope.namespace()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(storage_error)?;
    usize::try_from(count).map_err(numeric_error)
}

fn read_revision(connection: &Connection) -> Result<RegistryRevision, AgentMeshError> {
    let revision = connection
        .query_row(
            "SELECT revision FROM registry_meta WHERE singleton = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(storage_error)?;
    revision_from_sql(revision)
}

fn advance_revision(transaction: &Transaction<'_>) -> Result<RegistryRevision, AgentMeshError> {
    let current = transaction
        .query_row(
            "SELECT revision FROM registry_meta WHERE singleton = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(storage_error)?;
    let next = revision_from_sql(current)?.next()?;
    transaction
        .execute(
            "UPDATE registry_meta SET revision = ?1 WHERE singleton = 1",
            [revision_to_sql(next)?],
        )
        .map_err(storage_error)?;
    Ok(next)
}

fn revision_from_sql(value: i64) -> Result<RegistryRevision, AgentMeshError> {
    u64::try_from(value)
        .map(RegistryRevision::new)
        .map_err(numeric_error)
}

fn revision_to_sql(value: RegistryRevision) -> Result<i64, AgentMeshError> {
    i64::try_from(value.get()).map_err(numeric_error)
}

fn decode_service(document: &str) -> Result<RegisteredService, AgentMeshError> {
    let service: RegisteredService = serde_json::from_str(document).map_err(storage_error)?;
    service.validate().map_err(|error| {
        AgentMeshError::with_source(
            ErrorCode::StorageUnavailable,
            "The registry contains an invalid service document.",
            error,
        )
    })?;
    Ok(service)
}

fn storage_error(source: impl Into<agentmesh_error::BoxError>) -> AgentMeshError {
    AgentMeshError::with_source(
        ErrorCode::StorageUnavailable,
        "The registry storage operation failed.",
        source,
    )
}

fn numeric_error(source: impl Into<agentmesh_error::BoxError>) -> AgentMeshError {
    AgentMeshError::with_source(
        ErrorCode::StorageUnavailable,
        "The registry contains an invalid numeric value.",
        source,
    )
}

fn lock_error<T>(source: &std::sync::PoisonError<T>) -> AgentMeshError {
    storage_error(std::io::Error::other(source.to_string()))
}
