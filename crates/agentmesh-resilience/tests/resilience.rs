//! Resilience primitive integration tests.

use std::time::Duration;

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_resilience::{
    BoundedPriorityQueue, Bulkhead, Deadline, Idempotency, Priority, RetryBudget, RetryPolicy,
};

#[test]
fn unsafe_mutations_never_retry_and_backoff_is_bounded() {
    let policy = RetryPolicy::default();
    let error = AgentMeshError::new(ErrorCode::UpstreamTimeout, "timeout");
    assert!(!policy.should_retry(1, &error, Idempotency::Unsafe));
    assert!(policy.should_retry(1, &error, Idempotency::Verified));
    assert!(!policy.should_retry(3, &error, Idempotency::Safe));
    assert!(policy.delay(2, 42) <= Duration::from_millis(policy.max_delay_millis));
}

#[test]
fn deadline_caps_each_operation_by_total_budget() {
    let deadline = Deadline::new(100, 50).unwrap();
    assert_eq!(deadline.started_millis(), 100);
    assert_eq!(
        deadline.cap(Duration::from_millis(100), 125),
        Duration::from_millis(25)
    );
    assert!(deadline.expired(150));
}

#[test]
fn retry_budget_refills_without_exceeding_capacity() {
    let budget = RetryBudget::new(2, 1, 0).unwrap();
    assert!(budget.try_consume(0));
    assert!(budget.try_consume(0));
    assert!(!budget.try_consume(0));
    assert!(budget.try_consume(1_000));
}

#[test]
fn priority_queue_is_bounded_and_preserves_fifo_per_class() {
    let queue = BoundedPriorityQueue::new(3).unwrap();
    queue.push(Priority::Background, "background").unwrap();
    queue.push(Priority::High, "high-one").unwrap();
    queue.push(Priority::High, "high-two").unwrap();
    assert_eq!(queue.push(Priority::Normal, "overflow"), Err("overflow"));
    assert_eq!(queue.pop(), Some("high-one"));
    assert_eq!(queue.pop(), Some("high-two"));
    assert_eq!(queue.pop(), Some("background"));
}

#[test]
fn bulkhead_releases_on_drop_and_drain_fails_fast() {
    let bulkhead = Bulkhead::new(1).unwrap();
    let permit = bulkhead.try_acquire().unwrap();
    assert_eq!(
        bulkhead.try_acquire().err().unwrap().code(),
        ErrorCode::ConcurrencyLimited
    );
    drop(permit);
    assert_eq!(bulkhead.active(), 0);
    bulkhead.begin_drain();
    assert_eq!(
        bulkhead.try_acquire().err().unwrap().code(),
        ErrorCode::UpstreamUnavailable
    );
    bulkhead.resume();
    assert!(bulkhead.try_acquire().is_ok());
}
