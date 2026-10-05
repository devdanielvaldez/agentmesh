//! Safe recovery decisions for partial capability execution.

use serde::{Deserialize, Serialize};

use crate::{CapabilityPackage, ExecutionStatus, Idempotency, RecoveryStrategy};

/// Next action a runtime should take after a capability execution attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    /// No further action is needed for this execution attempt.
    Stop,
    /// Re-read external state before deciding whether to continue.
    Reconcile,
    /// Retry the failed operation under its declared idempotency guarantee.
    Retry,
    /// Continue from a known checkpoint.
    Resume,
    /// Run a declared compensating action.
    Compensate,
    /// Pause for an authorized human decision.
    HumanReview,
}

/// Facts needed to choose a bounded recovery action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryContext {
    /// Number of attempts already used.
    pub attempts_used: u8,
    /// Maximum allowed attempts for this execution.
    pub maximum_attempts: u8,
    /// Whether the original idempotency key can be reused.
    pub idempotency_key_available: bool,
    /// Whether the runtime cannot determine if the external effect occurred.
    pub external_state_unknown: bool,
    /// Whether the binding provides a tested compensating operation.
    pub compensation_available: bool,
}

/// Explainable recovery choice returned to the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryDecision {
    /// Selected next action.
    pub action: RecoveryAction,
    /// Stable reason code suitable for audit records.
    pub reason: &'static str,
}

/// Selects a safe next action from package strategy and observed execution state.
///
/// An unknown external state always requires reconciliation before replay,
/// resume, or compensation. This prevents a timeout from causing a duplicate
/// payment, message, reservation, or other external effect.
pub fn decide_recovery(
    package: &CapabilityPackage,
    status: ExecutionStatus,
    context: RecoveryContext,
) -> RecoveryDecision {
    if matches!(
        status,
        ExecutionStatus::Verified | ExecutionStatus::Compensated
    ) {
        return decision(RecoveryAction::Stop, "execution_already_terminal");
    }
    if status == ExecutionStatus::HumanReviewRequired {
        return decision(RecoveryAction::HumanReview, "human_review_already_required");
    }
    if context.external_state_unknown
        || matches!(
            status,
            ExecutionStatus::Completed
                | ExecutionStatus::PartiallyVerified
                | ExecutionStatus::ReconciliationRequired
        )
    {
        return decision(
            RecoveryAction::Reconcile,
            "external_state_must_be_reconciled",
        );
    }
    if status != ExecutionStatus::Failed {
        return decision(RecoveryAction::HumanReview, "unsupported_execution_state");
    }
    if context.maximum_attempts == 0 || context.attempts_used >= context.maximum_attempts {
        return decision(RecoveryAction::HumanReview, "attempt_limit_reached");
    }
    match package.contract.recovery {
        RecoveryStrategy::Reconcile => {
            decision(RecoveryAction::Reconcile, "package_requires_reconciliation")
        }
        RecoveryStrategy::Retry => {
            let safe_to_retry = match package.contract.idempotency {
                Idempotency::Guaranteed => true,
                Idempotency::KeyRequired => context.idempotency_key_available,
                Idempotency::NotGuaranteed => false,
            };
            if safe_to_retry {
                decision(RecoveryAction::Retry, "retry_is_idempotency_safe")
            } else {
                decision(RecoveryAction::Reconcile, "retry_safety_not_established")
            }
        }
        RecoveryStrategy::Resume => decision(RecoveryAction::Resume, "resume_from_checkpoint"),
        RecoveryStrategy::Compensate if context.compensation_available => {
            decision(RecoveryAction::Compensate, "compensation_is_available")
        }
        RecoveryStrategy::Compensate | RecoveryStrategy::HumanReview => {
            decision(RecoveryAction::HumanReview, "human_decision_required")
        }
    }
}

fn decision(action: RecoveryAction, reason: &'static str) -> RecoveryDecision {
    RecoveryDecision { action, reason }
}
