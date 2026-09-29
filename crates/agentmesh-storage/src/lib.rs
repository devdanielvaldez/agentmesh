//! Tenant-isolated persistence primitives with optimistic concurrency.

use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, RwLock},
};

use agentmesh_error::{AgentMeshError, ErrorCode};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// Maximum serialized document accepted by the generic store.
pub const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;

/// Tenant and namespace boundary attached to every storage operation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StorageScope {
    tenant: String,
    namespace: String,
}

impl StorageScope {
    /// Creates a validated scope.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] for an invalid tenant or namespace.
    pub fn new(
        tenant: impl Into<String>,
        namespace: impl Into<String>,
    ) -> Result<Self, AgentMeshError> {
        let value = Self {
            tenant: tenant.into(),
            namespace: namespace.into(),
        };
        if !valid_segment(&value.tenant) || !valid_segment(&value.namespace) {
            return Err(invalid("Storage scopes must use 1-128 safe characters."));
        }
        Ok(value)
    }

    /// Tenant identifier.
    pub fn tenant(&self) -> &str {
        &self.tenant
    }
    /// Namespace identifier.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }
}

/// Versioned, opaque JSON document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    /// Stable resource key.
    pub key: String,
    /// Monotonic revision.
    pub revision: u64,
    /// Resource body.
    pub value: serde_json::Value,
}

/// Write precondition used to prevent lost updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteCondition {
    /// Resource must not exist.
    Create,
    /// Resource must have this revision.
    Match(u64),
    /// Create or replace regardless of revision.
    Any,
}

/// Repository contract shared by local and cloud persistence adapters.
pub trait DocumentStore: Send + Sync {
    /// Reads one scoped document.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] for invalid keys or backend failures.
    fn get(&self, scope: &StorageScope, key: &str) -> Result<Option<Document>, AgentMeshError>;
    /// Writes one document atomically and returns its new revision.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] for invalid data, conflicts, or backend failures.
    fn put(
        &self,
        scope: &StorageScope,
        key: &str,
        value: serde_json::Value,
        condition: WriteCondition,
    ) -> Result<Document, AgentMeshError>;
    /// Deletes a matching revision.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] for conflicts or backend failures.
    fn delete(&self, scope: &StorageScope, key: &str, expected: u64) -> Result<(), AgentMeshError>;
    /// Lists a bounded page in stable key order.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] for invalid bounds or backend failures.
    fn list(
        &self,
        scope: &StorageScope,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<Document>, AgentMeshError>;
}

/// Deterministic process-local backend for tests and standalone use.
#[derive(Debug, Clone, Default)]
pub struct InMemoryStore {
    inner: Arc<RwLock<BTreeMap<(StorageScope, String), Document>>>,
}

impl DocumentStore for InMemoryStore {
    fn get(&self, scope: &StorageScope, key: &str) -> Result<Option<Document>, AgentMeshError> {
        validate_key(key)?;
        Ok(read(&self.inner)?
            .get(&(scope.clone(), key.into()))
            .cloned())
    }

    fn put(
        &self,
        scope: &StorageScope,
        key: &str,
        value: serde_json::Value,
        condition: WriteCondition,
    ) -> Result<Document, AgentMeshError> {
        validate_document(key, &value)?;
        let mut values = write(&self.inner)?;
        let map_key = (scope.clone(), key.to_owned());
        let current = values.get(&map_key);
        enforce(condition, current.map(|item| item.revision))?;
        let revision = current.map_or(1, |item| item.revision.saturating_add(1));
        let document = Document {
            key: key.into(),
            revision,
            value,
        };
        values.insert(map_key, document.clone());
        Ok(document)
    }

    fn delete(&self, scope: &StorageScope, key: &str, expected: u64) -> Result<(), AgentMeshError> {
        validate_key(key)?;
        let mut values = write(&self.inner)?;
        let map_key = (scope.clone(), key.to_owned());
        enforce(
            WriteCondition::Match(expected),
            values.get(&map_key).map(|item| item.revision),
        )?;
        values.remove(&map_key);
        Ok(())
    }

    fn list(
        &self,
        scope: &StorageScope,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<Document>, AgentMeshError> {
        validate_limit(limit)?;
        Ok(read(&self.inner)?
            .iter()
            .filter(|((item_scope, key), _)| item_scope == scope && key.starts_with(prefix))
            .take(limit)
            .map(|(_, value)| value.clone())
            .collect())
    }
}

/// `SQLite` backend for durable local deployments.
pub struct SqliteStore {
    connection: std::sync::Mutex<Connection>,
}

impl SqliteStore {
    /// Opens a database and applies the idempotent schema migration.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] when the database cannot be opened or migrated.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AgentMeshError> {
        let connection = Connection::open(path).map_err(storage)?;
        connection.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS documents (tenant TEXT NOT NULL, namespace TEXT NOT NULL, key TEXT NOT NULL, revision INTEGER NOT NULL, value TEXT NOT NULL, PRIMARY KEY (tenant, namespace, key));").map_err(storage)?;
        Ok(Self {
            connection: std::sync::Mutex::new(connection),
        })
    }
}

impl DocumentStore for SqliteStore {
    fn get(&self, scope: &StorageScope, key: &str) -> Result<Option<Document>, AgentMeshError> {
        validate_key(key)?;
        let connection = self.connection.lock().map_err(|_| unavailable())?;
        connection
            .query_row(
                "SELECT revision, value FROM documents WHERE tenant=?1 AND namespace=?2 AND key=?3",
                params![scope.tenant(), scope.namespace(), key],
                |row| {
                    let revision = u64::try_from(row.get::<_, i64>(0)?).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?;
                    let raw = row.get::<_, String>(1)?;
                    let value = serde_json::from_str(&raw).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            raw.len(),
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                    Ok(Document {
                        key: key.into(),
                        revision,
                        value,
                    })
                },
            )
            .optional()
            .map_err(storage)
    }

    fn put(
        &self,
        scope: &StorageScope,
        key: &str,
        value: serde_json::Value,
        condition: WriteCondition,
    ) -> Result<Document, AgentMeshError> {
        validate_document(key, &value)?;
        let raw = serde_json::to_string(&value).map_err(storage)?;
        let mut connection = self.connection.lock().map_err(|_| unavailable())?;
        let transaction = connection.transaction().map_err(storage)?;
        let current_sql: Option<i64> = transaction
            .query_row(
                "SELECT revision FROM documents WHERE tenant=?1 AND namespace=?2 AND key=?3",
                params![scope.tenant(), scope.namespace(), key],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        let current = current_sql
            .map(u64::try_from)
            .transpose()
            .map_err(storage)?;
        enforce(condition, current)?;
        let revision = current.map_or(1, |value| value.saturating_add(1));
        let sql_revision = i64::try_from(revision).map_err(storage)?;
        transaction.execute("INSERT INTO documents(tenant, namespace, key, revision, value) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(tenant,namespace,key) DO UPDATE SET revision=excluded.revision,value=excluded.value", params![scope.tenant(), scope.namespace(), key, sql_revision, raw]).map_err(storage)?;
        transaction.commit().map_err(storage)?;
        Ok(Document {
            key: key.into(),
            revision,
            value,
        })
    }

    fn delete(&self, scope: &StorageScope, key: &str, expected: u64) -> Result<(), AgentMeshError> {
        let connection = self.connection.lock().map_err(|_| unavailable())?;
        let expected = i64::try_from(expected).map_err(storage)?;
        let changed = connection
            .execute(
                "DELETE FROM documents WHERE tenant=?1 AND namespace=?2 AND key=?3 AND revision=?4",
                params![scope.tenant(), scope.namespace(), key, expected],
            )
            .map_err(storage)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(conflict())
        }
    }

    fn list(
        &self,
        scope: &StorageScope,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<Document>, AgentMeshError> {
        validate_limit(limit)?;
        let connection = self.connection.lock().map_err(|_| unavailable())?;
        let mut statement = connection.prepare("SELECT key, revision, value FROM documents WHERE tenant=?1 AND namespace=?2 AND key LIKE ?3 ORDER BY key LIMIT ?4").map_err(storage)?;
        let pattern = format!("{}%", prefix.replace('%', "\\%").replace('_', "\\_"));
        let sql_limit = i64::try_from(limit).map_err(storage)?;
        let rows = statement
            .query_map(
                params![scope.tenant(), scope.namespace(), pattern, sql_limit],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(storage)?;
        rows.map(|row| {
            let (key, revision, raw) = row.map_err(storage)?;
            let revision = u64::try_from(revision).map_err(storage)?;
            let value = serde_json::from_str(&raw).map_err(storage)?;
            Ok(Document {
                key,
                revision,
                value,
            })
        })
        .collect()
    }
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}
fn validate_key(key: &str) -> Result<(), AgentMeshError> {
    if valid_segment(key) || (key.len() <= 256 && key.split('/').all(valid_segment)) {
        Ok(())
    } else {
        Err(invalid("The storage key is invalid."))
    }
}
fn validate_document(key: &str, value: &serde_json::Value) -> Result<(), AgentMeshError> {
    validate_key(key)?;
    if serde_json::to_vec(value).map_err(storage)?.len() > MAX_DOCUMENT_BYTES {
        Err(AgentMeshError::new(
            ErrorCode::PayloadTooLarge,
            "The storage document exceeds its size limit.",
        ))
    } else {
        Ok(())
    }
}
fn validate_limit(limit: usize) -> Result<(), AgentMeshError> {
    if (1..=50_000).contains(&limit) {
        Ok(())
    } else {
        Err(invalid("Storage list limits must be between 1 and 50000."))
    }
}
fn enforce(condition: WriteCondition, current: Option<u64>) -> Result<(), AgentMeshError> {
    match condition {
        WriteCondition::Any => Ok(()),
        WriteCondition::Create if current.is_none() => Ok(()),
        WriteCondition::Match(expected) if current == Some(expected) => Ok(()),
        _ => Err(conflict()),
    }
}
fn invalid(message: &str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::SchemaInvalid, message)
}
fn conflict() -> AgentMeshError {
    AgentMeshError::new(
        ErrorCode::Conflict,
        "The stored resource revision does not match.",
    )
}
fn unavailable() -> AgentMeshError {
    AgentMeshError::new(
        ErrorCode::StorageUnavailable,
        "The storage backend is unavailable.",
    )
}
fn storage(error: impl std::error::Error + Send + Sync + 'static) -> AgentMeshError {
    AgentMeshError::with_source(
        ErrorCode::StorageUnavailable,
        "The storage operation failed.",
        error,
    )
}
fn read<T>(lock: &RwLock<T>) -> Result<std::sync::RwLockReadGuard<'_, T>, AgentMeshError> {
    lock.read().map_err(|_| unavailable())
}
fn write<T>(lock: &RwLock<T>) -> Result<std::sync::RwLockWriteGuard<'_, T>, AgentMeshError> {
    lock.write().map_err(|_| unavailable())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn contract(store: &dyn DocumentStore) {
        let a = StorageScope::new("acme", "prod").unwrap();
        let b = StorageScope::new("other", "prod").unwrap();
        let first = store
            .put(
                &a,
                "routes/main",
                json!({"weight": 1}),
                WriteCondition::Create,
            )
            .unwrap();
        assert_eq!(first.revision, 1);
        assert!(store.get(&b, "routes/main").unwrap().is_none());
        assert!(
            store
                .put(&a, "routes/main", json!({}), WriteCondition::Match(9))
                .is_err()
        );
        assert_eq!(
            store
                .put(
                    &a,
                    "routes/main",
                    json!({"weight": 2}),
                    WriteCondition::Match(1)
                )
                .unwrap()
                .revision,
            2
        );
        assert_eq!(store.list(&a, "routes/", 10).unwrap().len(), 1);
        store.delete(&a, "routes/main", 2).unwrap();
    }

    #[test]
    fn memory_contract() {
        contract(&InMemoryStore::default());
    }
    #[test]
    fn sqlite_contract() {
        let file = tempfile::NamedTempFile::new().unwrap();
        contract(&SqliteStore::open(file.path()).unwrap());
    }
}
