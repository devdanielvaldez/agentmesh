//! Task affinity and lifecycle integration tests.

use agentmesh_error::ErrorCode;
use agentmesh_registry::{EndpointId, RegistryScope};
use agentmesh_tasks::{InMemoryTaskStore, TaskAccess, TaskRecord, TaskStatus};

#[test]
fn task_affinity_owner_and_lifecycle_are_preserved() {
    let store = InMemoryTaskStore::new();
    let scope = RegistryScope::new("acme", "prod").unwrap();
    let endpoint = EndpointId::new();
    let created = store
        .create(
            TaskRecord::new(scope.clone(), "alice", endpoint, "upstream-42", 1_000, 0).unwrap(),
            0,
        )
        .unwrap();
    let alice = TaskAccess {
        scope: &scope,
        principal: "alice",
        administer: false,
    };
    assert_eq!(
        store.get(&alice, created.id, 1).unwrap().backend_endpoint,
        endpoint
    );
    assert_eq!(
        store.get(&alice, created.id, 1).unwrap().backend_task_id(),
        "upstream-42"
    );
    let running = store
        .transition(&alice, created.id, created.revision, TaskStatus::Running, 2)
        .unwrap();
    let succeeded = store
        .transition(
            &alice,
            created.id,
            running.revision,
            TaskStatus::Succeeded,
            3,
        )
        .unwrap();
    assert!(succeeded.status.terminal());
    assert_eq!(store.drain_events().len(), 3);
}

#[test]
fn cross_owner_scope_stale_and_terminal_access_fail_closed() {
    let store = InMemoryTaskStore::new();
    let scope = RegistryScope::new("acme", "prod").unwrap();
    let created = store
        .create(
            TaskRecord::new(scope.clone(), "alice", EndpointId::new(), "backend", 100, 0).unwrap(),
            0,
        )
        .unwrap();
    let bob = TaskAccess {
        scope: &scope,
        principal: "bob",
        administer: false,
    };
    assert_eq!(
        store.get(&bob, created.id, 1).unwrap_err().code(),
        ErrorCode::PermissionDenied
    );
    let admin = TaskAccess {
        scope: &scope,
        principal: "operator",
        administer: true,
    };
    let cancelled = store
        .cancel(&admin, created.id, created.revision, 2)
        .unwrap();
    assert_eq!(
        store
            .cancel(&admin, created.id, created.revision, 3)
            .unwrap_err()
            .code(),
        ErrorCode::Conflict
    );
    assert_eq!(
        store
            .transition(
                &admin,
                created.id,
                cancelled.revision,
                TaskStatus::Running,
                4
            )
            .unwrap_err()
            .code(),
        ErrorCode::Conflict
    );
    let other = RegistryScope::new("other", "prod").unwrap();
    let access = TaskAccess {
        scope: &other,
        principal: "alice",
        administer: true,
    };
    assert_eq!(
        store.get(&access, created.id, 5).unwrap_err().code(),
        ErrorCode::CapabilityNotFound
    );
}

#[test]
fn expired_terminal_tasks_are_hidden_and_purged_in_batches() {
    let store = InMemoryTaskStore::new();
    let scope = RegistryScope::new("acme", "prod").unwrap();
    let created = store
        .create(
            TaskRecord::new(scope.clone(), "alice", EndpointId::new(), "backend", 10, 0).unwrap(),
            0,
        )
        .unwrap();
    let access = TaskAccess {
        scope: &scope,
        principal: "alice",
        administer: false,
    };
    let cancelled = store
        .cancel(&access, created.id, created.revision, 1)
        .unwrap();
    assert_eq!(
        store.get(&access, cancelled.id, 10).unwrap_err().code(),
        ErrorCode::CapabilityNotFound
    );
    assert_eq!(store.purge_expired(10, 1), 1);
}

#[test]
fn backend_identifiers_are_redacted_from_debug() {
    let scope = RegistryScope::new("acme", "prod").unwrap();
    let record =
        TaskRecord::new(scope, "alice", EndpointId::new(), "super-secret-id", 10, 0).unwrap();
    let debug = format!("{record:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains("super-secret-id"));
}
