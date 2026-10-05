//! Governed execution of portable capability packages.

use std::collections::BTreeSet;

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_policy::{
    CapabilityAdmissionDecision, CapabilityAdmissionPolicy, PolicyEffect, admit_capability,
};
use agentmesh_protocol::{
    CapabilityPackage, EvidenceVerification, ExecutionEvidence, ExecutionReceipt, ExecutionStatus,
    ImplementationBinding, ImplementationKind, verify_execution_receipt,
};
use serde_json::Value;

/// Output from one concrete implementation attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityAttempt {
    /// Runtime status before contract evidence is evaluated.
    pub status: ExecutionStatus,
    /// Evidence references returned by the implementation.
    pub evidence: Vec<ExecutionEvidence>,
    /// Policy and approval decision identifiers to preserve in the receipt.
    pub policy_decisions: Vec<String>,
}

/// Binding executor supplied by MCP, Teach, API, A2A, CLI, or another adapter.
pub trait CapabilityExecutor: Send + Sync {
    /// Runs one selected implementation with validated input data.
    fn execute(
        &self,
        binding: &ImplementationBinding,
        inputs: &Value,
        execution_id: &str,
    ) -> Result<CapabilityAttempt, AgentMeshError>;
}

/// Runtime constraints for capability admission and implementation selection.
#[derive(Debug, Clone)]
pub struct CapabilityRuntimeConfig {
    /// Static package admission policy.
    pub admission: CapabilityAdmissionPolicy,
    /// Binding IDs installed and usable in this runtime.
    pub available_bindings: BTreeSet<String>,
    /// Preferred binding kinds, highest priority first.
    pub preferred_kinds: Vec<ImplementationKind>,
}

/// Result of one capability attempt with verified evidence coverage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityExecutionResult {
    /// Admission decision applied before invoking a provider.
    pub admission: CapabilityAdmissionDecision,
    /// Portable execution receipt.
    pub receipt: ExecutionReceipt,
    /// Evidence coverage against the capability contract.
    pub verification: EvidenceVerification,
}

/// Executes a capability only after input validation, admission, and binding resolution.
///
/// Approval requests return [`ErrorCode::ApprovalRequired`] before the executor
/// is called. Provider signatures and approval records are resolved by their
/// respective trust and policy adapters before this entry point is invoked.
///
/// # Errors
///
/// Returns an error for invalid inputs/packages, denied admission, required
/// approval, missing bindings, or implementation failure.
pub fn execute_capability(
    package: &CapabilityPackage,
    inputs: &Value,
    execution_id: &str,
    config: &CapabilityRuntimeConfig,
    executor: &dyn CapabilityExecutor,
) -> Result<CapabilityExecutionResult, AgentMeshError> {
    if execution_id.trim().is_empty() || execution_id.len() > 256 {
        return Err(AgentMeshError::new(
            ErrorCode::InvalidRequest,
            "The capability execution identifier is invalid.",
        ));
    }
    package.validate().map_err(|_| {
        AgentMeshError::new(
            ErrorCode::SchemaInvalid,
            "The capability package is invalid.",
        )
    })?;
    validate_inputs(inputs, &package.contract.inputs)?;
    let admission = admit_capability(package, &config.admission);
    match admission.effect {
        PolicyEffect::Deny => {
            return Err(AgentMeshError::new(
                ErrorCode::PolicyDenied,
                "The capability is not admitted by runtime policy.",
            ));
        }
        PolicyEffect::RequireApproval => {
            return Err(AgentMeshError::new(
                ErrorCode::ApprovalRequired,
                "The capability requires approval before execution.",
            ));
        }
        PolicyEffect::Allow => {}
    }
    let binding = agentmesh_protocol::select_implementation(
        package,
        &config.available_bindings,
        &config.preferred_kinds,
    )
    .map_err(|_| {
        AgentMeshError::new(
            ErrorCode::CapabilityNotFound,
            "No compatible capability implementation is available.",
        )
    })?;
    let attempt = executor.execute(binding, inputs, execution_id)?;
    let mut receipt = ExecutionReceipt {
        execution_id: execution_id.into(),
        capability_id: package.capability.id.clone(),
        capability_version: package.capability.version.clone(),
        implementation_id: binding.id.clone(),
        status: attempt.status,
        evidence: attempt.evidence,
        policy_decisions: attempt.policy_decisions,
        package_digest: package.content_digest().ok(),
        extensions: Default::default(),
    };
    let verification = verify_execution_receipt(package, &receipt);
    if verification.verified {
        receipt.status = ExecutionStatus::Verified;
    } else if receipt.status == ExecutionStatus::Completed
        && !verification.verified_claims.is_empty()
    {
        receipt.status = ExecutionStatus::PartiallyVerified;
    }
    Ok(CapabilityExecutionResult {
        admission,
        receipt,
        verification,
    })
}

fn validate_inputs(inputs: &Value, schema: &Value) -> Result<(), AgentMeshError> {
    let Some(input_object) = inputs.as_object() else {
        return Err(input_error());
    };
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err(input_error());
    }
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for property in required.iter().filter_map(Value::as_str) {
            if !input_object.contains_key(property) {
                return Err(input_error());
            }
        }
    }
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        if schema.get("additionalProperties").and_then(Value::as_bool) == Some(false)
            && input_object.keys().any(|key| !properties.contains_key(key))
        {
            return Err(input_error());
        }
        for (name, value) in input_object {
            let Some(property_schema) = properties.get(name) else {
                continue;
            };
            if let Some(expected) = property_schema.get("type").and_then(Value::as_str) {
                let matches = match expected {
                    "object" => value.is_object(),
                    "array" => value.is_array(),
                    "string" => value.is_string(),
                    "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
                    "number" => value.is_number(),
                    "boolean" => value.is_boolean(),
                    _ => false,
                };
                if !matches {
                    return Err(input_error());
                }
            }
        }
    }
    Ok(())
}

fn input_error() -> AgentMeshError {
    AgentMeshError::new(
        ErrorCode::SchemaInvalid,
        "Capability inputs do not match the declared input contract.",
    )
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use agentmesh_policy::{CapabilityAdmissionPolicy, Risk};
    use agentmesh_protocol::{
        CAPABILITY_PACKAGE_VERSION, CapabilityContract, CapabilityDefinition, EffectKind,
        EvidenceClaim, EvidenceType, Idempotency, ImplementationKind, Provenance, ProvenanceSource,
        RecoveryStrategy,
    };
    use serde_json::json;

    use super::*;

    struct FixtureExecutor;

    impl CapabilityExecutor for FixtureExecutor {
        fn execute(
            &self,
            binding: &ImplementationBinding,
            _inputs: &Value,
            _execution_id: &str,
        ) -> Result<CapabilityAttempt, AgentMeshError> {
            Ok(CapabilityAttempt {
                status: ExecutionStatus::Completed,
                evidence: vec![ExecutionEvidence {
                    claim_id: "result_present".into(),
                    evidence_type: EvidenceType::StateAssertion,
                    reference: "assertion:result-present".into(),
                    digest: None,
                }],
                policy_decisions: vec![format!("binding:{}", binding.id)],
            })
        }
    }

    fn package() -> CapabilityPackage {
        let mut package = CapabilityPackage {
            schema_version: CAPABILITY_PACKAGE_VERSION.into(),
            capability: CapabilityDefinition {
                id: "demo.lookup".into(),
                version: "1.0.0".into(),
                intent: "Look up a demo record".into(),
            },
            contract: CapabilityContract {
                requires: Vec::new(),
                inputs: json!({
                    "type":"object",
                    "properties":{"query":{"type":"string"}},
                    "required":["query"],
                    "additionalProperties":false
                }),
                outputs: json!({"type":"object"}),
                success_evidence: vec![EvidenceClaim {
                    id: "result_present".into(),
                    assertion: "lookup result exists".into(),
                    accepted_types: vec![EvidenceType::StateAssertion],
                }],
                effects: vec![EffectKind::ReadOnly],
                idempotency: Idempotency::Guaranteed,
                recovery: RecoveryStrategy::Retry,
            },
            authority: Default::default(),
            implementations: vec![ImplementationBinding {
                id: "demo-mcp".into(),
                kind: ImplementationKind::Mcp,
                reference: "mcp://demo/lookup".into(),
                configuration: BTreeMap::new(),
            }],
            provenance: Some(Provenance {
                publisher: "demo.example".into(),
                package_digest: String::new(),
                source: ProvenanceSource::Authored,
                signature: None,
            }),
            extensions: BTreeMap::new(),
        };
        let digest = package.content_digest().expect("digest");
        package.provenance.as_mut().unwrap().package_digest = digest;
        package
    }

    fn config() -> CapabilityRuntimeConfig {
        CapabilityRuntimeConfig {
            admission: CapabilityAdmissionPolicy {
                allowed_effects: BTreeSet::from([EffectKind::ReadOnly]),
                granted_permissions: BTreeSet::new(),
                maximum_risk: Risk::Low,
                approval_at_or_above: Risk::Medium,
                require_provenance: true,
            },
            available_bindings: BTreeSet::from(["demo-mcp".into()]),
            preferred_kinds: vec![ImplementationKind::Mcp],
        }
    }

    #[test]
    fn runtime_admits_validates_selects_executes_and_verifies() {
        let result = execute_capability(
            &package(),
            &json!({"query":"alpha"}),
            "exec-1",
            &config(),
            &FixtureExecutor,
        )
        .expect("capability execution");
        assert_eq!(result.receipt.status, ExecutionStatus::Verified);
        assert!(result.verification.verified);
    }

    #[test]
    fn runtime_rejects_bad_input_before_calling_executor() {
        let error = execute_capability(
            &package(),
            &json!({"query":42}),
            "exec-2",
            &config(),
            &FixtureExecutor,
        )
        .expect_err("input type mismatch");
        assert_eq!(error.code(), ErrorCode::SchemaInvalid);
    }
}
