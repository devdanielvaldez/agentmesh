//! Portable execution receipts and evidence verification.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{CapabilityPackage, EvidenceType};

/// Terminal or suspended state reported for one capability execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    /// The selected implementation stopped normally; success is not implied.
    Completed,
    /// Every declared success claim has acceptable evidence.
    Verified,
    /// Some, but not all, success claims have acceptable evidence.
    PartiallyVerified,
    /// The external system must be reconciled before another action.
    ReconciliationRequired,
    /// Execution failed before a verified outcome.
    Failed,
    /// A declared compensating action was completed.
    Compensated,
    /// Execution is paused for an authorized human decision.
    HumanReviewRequired,
}

/// One referenceable item of execution evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionEvidence {
    /// Success claim this evidence is intended to substantiate.
    pub claim_id: String,
    /// Evidence class used by the contract.
    pub evidence_type: EvidenceType,
    /// Opaque provider receipt, signed object, or retained audit reference.
    pub reference: String,
    /// Optional integrity digest of the retained evidence artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

/// Portable receipt produced for every capability execution attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionReceipt {
    /// Globally unique execution attempt identifier.
    pub execution_id: String,
    /// Capability identity as resolved at execution time.
    pub capability_id: String,
    /// Exact capability version as resolved at execution time.
    pub capability_version: String,
    /// Chosen implementation binding identifier.
    pub implementation_id: String,
    /// Runtime's execution state.
    pub status: ExecutionStatus,
    /// Evidence retained by the runtime or provider.
    #[serde(default)]
    pub evidence: Vec<ExecutionEvidence>,
    /// Sanitized policy and approval decision identifiers.
    #[serde(default)]
    pub policy_decisions: Vec<String>,
    /// Digest of the resolved package when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_digest: Option<String>,
    /// Forward-compatible runtime-specific receipt fields.
    #[serde(flatten)]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

/// Result of checking a receipt against its package contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceVerification {
    /// Whether every success claim was substantiated by an accepted evidence type.
    pub verified: bool,
    /// Claim IDs supported by the receipt.
    pub verified_claims: BTreeSet<String>,
    /// Required claim IDs not supported by the receipt.
    pub missing_claims: BTreeSet<String>,
    /// Structural receipt errors that make it unusable for verification.
    pub errors: Vec<String>,
}

/// Verifies a receipt against the portable capability contract.
///
/// This validates evidence type and coverage. Signature verification and the
/// provider-specific retrieval of a receipt are deliberately delegated to
/// later bindings; a runtime must not report `verified` until both layers pass.
pub fn verify_execution_receipt(
    package: &CapabilityPackage,
    receipt: &ExecutionReceipt,
) -> EvidenceVerification {
    let mut errors = Vec::new();
    if package.validate().is_err() {
        errors.push("invalid_capability_package".into());
    }
    if receipt.execution_id.trim().is_empty() {
        errors.push("missing_execution_id".into());
    }
    if receipt.capability_id != package.capability.id
        || receipt.capability_version != package.capability.version
    {
        errors.push("capability_identity_mismatch".into());
    }
    if !package
        .implementations
        .iter()
        .any(|binding| binding.id == receipt.implementation_id)
    {
        errors.push("unknown_implementation".into());
    }

    let claims: BTreeMap<_, _> = package
        .contract
        .success_evidence
        .iter()
        .map(|claim| (claim.id.as_str(), claim))
        .collect();
    let mut verified_claims = BTreeSet::new();
    for evidence in &receipt.evidence {
        if evidence.reference.trim().is_empty() {
            errors.push(format!("empty_evidence_reference:{}", evidence.claim_id));
            continue;
        }
        let Some(claim) = claims.get(evidence.claim_id.as_str()) else {
            errors.push(format!("unknown_evidence_claim:{}", evidence.claim_id));
            continue;
        };
        if claim.accepted_types.contains(&evidence.evidence_type) {
            verified_claims.insert(evidence.claim_id.clone());
        }
    }
    let missing_claims: BTreeSet<String> = claims
        .keys()
        .filter(|claim_id| !verified_claims.contains::<str>(*claim_id))
        .map(|claim_id| (*claim_id).to_owned())
        .collect();
    let verified = errors.is_empty() && missing_claims.is_empty();
    EvidenceVerification {
        verified,
        verified_claims,
        missing_claims,
        errors,
    }
}
