//! Adapter from the learned Workflow IR to the portable AMCP package contract.

use std::collections::BTreeMap;

use agentmesh_protocol::{
    CapabilityContract, CapabilityDefinition, CapabilityPackage, EffectKind, EvidenceClaim,
    EvidenceType, Idempotency, ImplementationBinding, ImplementationKind, RecoveryStrategy,
};
use serde_json::{Map, Value, json};

use crate::{TeachError, Workflow, validate_workflow};

/// Compiles a validated Teach workflow into a portable capability package.
///
/// The generated package preserves the workflow ID, typed inputs, outputs,
/// postcondition evidence, coarse side effects, and implementation reference.
/// It deliberately rejects workflows without explicit success assertions: a
/// final step count is not sufficient evidence of accomplishing the goal.
///
/// # Errors
///
/// Returns a Teach validation error for invalid workflows or workflows without
/// a declared success condition.
pub fn compile_workflow_capability(workflow: &Workflow) -> Result<CapabilityPackage, TeachError> {
    validate_workflow(workflow)?;
    if workflow.success.is_empty() {
        return Err(TeachError::Validation(
            "portable capability export requires at least one success assertion".into(),
        ));
    }

    let mut properties = Map::new();
    let mut required = Vec::new();
    for (name, input) in &workflow.inputs {
        let mut schema = Map::new();
        schema.insert(
            "type".into(),
            Value::String(input_type_name(input.input_type).into()),
        );
        if let Some(default) = &input.default {
            schema.insert("default".into(), default.clone());
        }
        if input.is_required() {
            required.push(Value::String(name.clone()));
        }
        properties.insert(name.clone(), Value::Object(schema));
    }
    let input_schema = json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": required,
        "additionalProperties": false
    });

    let output_properties: Map<String, Value> = workflow
        .outputs
        .keys()
        .map(|name| (name.clone(), json!({})))
        .collect();
    let output_schema = json!({
        "type": "object",
        "properties": Value::Object(output_properties),
        "required": workflow.outputs.keys().cloned().map(Value::String).collect::<Vec<_>>(),
        "additionalProperties": false
    });

    let success_evidence = workflow
        .success
        .iter()
        .enumerate()
        .map(|(index, assertion)| EvidenceClaim {
            id: format!("success_{}", index + 1),
            assertion: assertion_description(assertion),
            accepted_types: vec![EvidenceType::StateAssertion],
        })
        .collect();

    let mut effects = Vec::new();
    let has_browser = workflow
        .steps
        .iter()
        .any(|step| step.op.starts_with("browser.") || step.op.starts_with("ui."));
    let has_mutation = workflow.steps.iter().any(|step| {
        matches!(
            step.op.as_str(),
            "ui.fill"
                | "ui.click"
                | "ui.activate"
                | "ui.press"
                | "ui.select"
                | "ui.drag"
                | "ui.drop"
                | "file.choose"
                | "file.upload"
                | "file.download"
                | "file.save"
        )
    });
    if has_browser {
        effects.push(EffectKind::NetworkEgress);
    }
    if has_mutation {
        effects.push(EffectKind::DataWrite);
    }
    if effects.is_empty() {
        effects.push(EffectKind::ReadOnly);
    }

    let mut permissions = Vec::new();
    if has_browser {
        permissions.push("browser.control".into());
        permissions.push("network.egress".into());
    }
    if workflow
        .steps
        .iter()
        .any(|step| step.op.starts_with("file."))
    {
        permissions.push("filesystem.write".into());
    }

    let recovery = workflow
        .recovery
        .as_ref()
        .map_or(RecoveryStrategy::HumanReview, |recovery| {
            if recovery.max_attempts > 1 {
                RecoveryStrategy::Reconcile
            } else {
                RecoveryStrategy::HumanReview
            }
        });
    let package = CapabilityPackage {
        schema_version: agentmesh_protocol::CAPABILITY_PACKAGE_VERSION.into(),
        capability: CapabilityDefinition {
            id: workflow.id.clone(),
            version: workflow.version.clone(),
            intent: if workflow.description.trim().is_empty() {
                workflow.id.clone()
            } else {
                workflow.description.clone()
            },
        },
        contract: CapabilityContract {
            requires: Vec::new(),
            inputs: input_schema,
            outputs: output_schema,
            success_evidence,
            effects,
            idempotency: Idempotency::NotGuaranteed,
            recovery,
        },
        authority: agentmesh_protocol::AuthorityRequirements {
            permissions,
            approvals: Vec::new(),
            constraints: BTreeMap::new(),
        },
        implementations: vec![ImplementationBinding {
            id: "teach-workflow".into(),
            kind: ImplementationKind::AgentmeshWorkflow,
            reference: format!("workflow://{}", workflow.id),
            configuration: BTreeMap::new(),
        }],
        provenance: None,
        extensions: BTreeMap::new(),
    };
    package
        .validate()
        .map_err(|error| TeachError::Validation(error.to_string()))?;
    Ok(package)
}

fn input_type_name(input_type: crate::InputType) -> &'static str {
    match input_type {
        crate::InputType::String | crate::InputType::Datetime => "string",
        crate::InputType::Integer => "integer",
        crate::InputType::Number => "number",
        crate::InputType::Boolean => "boolean",
        crate::InputType::Array => "array",
        crate::InputType::Object => "object",
    }
}

fn assertion_description(assertion: &crate::WorkflowAssertion) -> String {
    let target = assertion
        .target
        .as_ref()
        .and_then(|target| {
            target
                .semantic
                .as_deref()
                .or(target.accessible_name.as_deref())
        })
        .unwrap_or("page state");
    format!("{} holds for {target}", assertion.op)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_typed_workflow_and_requires_success_evidence() {
        let workflow = crate::parse_workflow(
            r#"
version: "1.0"
id: demo.search
description: Search demo records
runtime: browser
inputs:
  query:
    type: string
    required: true
steps:
  - id: open
    op: browser.navigate
    url: https://example.com
  - id: search
    op: ui.fill
    target:
      semantic: search_box
      role: textbox
    value: "{{ inputs.query }}"
outputs:
  results:
    from: steps.search.result
success:
  - op: assert.exists
    target:
      semantic: search_results
"#,
        )
        .expect("valid workflow yaml");
        let package = compile_workflow_capability(&workflow).expect("portable package");
        assert_eq!(package.capability.id, "demo.search");
        assert_eq!(package.contract.inputs["required"][0], "query");
        assert_eq!(package.contract.outputs["properties"]["results"], json!({}));
        assert!(package.contract.effects.contains(&EffectKind::DataWrite));

        let mut no_verifier = workflow;
        no_verifier.success.clear();
        assert!(compile_workflow_capability(&no_verifier).is_err());
    }
}
