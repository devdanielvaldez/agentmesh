//! Audit redaction, chaining, delivery policy, retry, and acknowledgement tests.

use std::collections::BTreeMap;

use agentmesh_audit::{AuditEvent, AuditOutcome, AuditSink, DeliveryPolicy, InMemoryAuditOutbox};
use agentmesh_error::ErrorCode;
use agentmesh_registry::RegistryScope;

fn event(action: &str) -> AuditEvent {
    AuditEvent::new(
        RegistryScope::new("acme", "prod").unwrap(),
        "alice",
        action,
        "tool/weather",
        AuditOutcome::Success,
        Some(2),
        Some(3),
        Some("trace-1".into()),
        10,
        &BTreeMap::from([
            ("api_token".into(), "secret".into()),
            ("region".into(), "east".into()),
        ]),
    )
    .unwrap()
}

#[test]
fn records_are_redacted_hash_chained_and_acknowledged() {
    let outbox = InMemoryAuditOutbox::new(10).unwrap();
    outbox
        .record(event("tool.call"), DeliveryPolicy::Required)
        .unwrap();
    outbox
        .record(event("task.create"), DeliveryPolicy::Required)
        .unwrap();
    let records = outbox.pending(10);
    assert_eq!(records[0].event.metadata["api_token"], "[REDACTED]");
    InMemoryAuditOutbox::verify(&records).unwrap();
    outbox.mark_attempt(1, Some("EXPORT_TIMEOUT")).unwrap();
    assert_eq!(outbox.pending(1)[0].attempts, 1);
    assert_eq!(outbox.acknowledge_through(1), 1);
    assert_eq!(outbox.pending(10)[0].sequence, 2);
}

#[test]
fn saturation_obeys_required_and_best_effort_policies() {
    let outbox = InMemoryAuditOutbox::new(1).unwrap();
    outbox
        .record(event("first"), DeliveryPolicy::Required)
        .unwrap();
    assert_eq!(
        outbox
            .record(event("required"), DeliveryPolicy::Required)
            .unwrap_err()
            .code(),
        ErrorCode::StorageUnavailable
    );
    assert_eq!(
        outbox
            .record(event("best_effort"), DeliveryPolicy::BestEffort)
            .unwrap(),
        None
    );
    assert_eq!(outbox.dropped(), 1);
}

#[test]
fn tampering_is_detected() {
    let outbox = InMemoryAuditOutbox::new(2).unwrap();
    outbox
        .record(event("first"), DeliveryPolicy::Required)
        .unwrap();
    let mut records = outbox.pending(2);
    records[0].event.action = "tampered".into();
    assert_eq!(
        InMemoryAuditOutbox::verify(&records).unwrap_err().code(),
        ErrorCode::StorageUnavailable
    );
}
