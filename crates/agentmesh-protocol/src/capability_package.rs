//! Portable AgentMesh Capability Package (AMCP) contracts.
//!
//! A capability describes an outcome an agent can achieve.  It deliberately
//! does not prescribe whether that outcome is achieved by MCP, an API, a
//! browser workflow, a CLI, or another agent; those are implementations bound
//! to the same contract.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The first supported Capability Package schema revision.
pub const CAPABILITY_PACKAGE_VERSION: &str = "amcp/0.1";

/// A portable, serializable declaration of an agent capability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityPackage {
    /// Schema revision used by this document.
    pub schema_version: String,
    /// Stable identity and intent of the capability.
    pub capability: CapabilityDefinition,
    /// Observable contract offered by the capability.
    pub contract: CapabilityContract,
    /// Authority the capability needs before it may execute.
    #[serde(default)]
    pub authority: AuthorityRequirements,
    /// Concrete ways to execute the capability.
    pub implementations: Vec<ImplementationBinding>,
    /// Supply-chain information about the package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
    /// Forward-compatible vendor or profile fields.
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

impl CapabilityPackage {
    /// Validates invariants that every compatible runtime can enforce locally.
    ///
    /// # Errors
    ///
    /// Returns a stable, human-readable validation error when the package is
    /// structurally unsafe or ambiguous.
    pub fn validate(&self) -> Result<(), CapabilityValidationError> {
        if self.schema_version != CAPABILITY_PACKAGE_VERSION {
            return Err(CapabilityValidationError::UnsupportedSchemaVersion(
                self.schema_version.clone(),
            ));
        }
        validate_capability_id(&self.capability.id)?;
        if self.capability.version.trim().is_empty()
            || self.capability.version == "."
            || self.capability.version == ".."
            || !self.capability.version.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+' | b'_')
            })
        {
            return Err(CapabilityValidationError::InvalidField(
                "capability.version must be a non-empty safe version identifier".into(),
            ));
        }
        if self.capability.intent.trim().is_empty() {
            return Err(CapabilityValidationError::InvalidField(
                "capability.intent must not be empty".into(),
            ));
        }
        validate_object_schema("contract.inputs", &self.contract.inputs)?;
        validate_object_schema("contract.outputs", &self.contract.outputs)?;
        validate_unique_non_empty(
            "contract.requires",
            self.contract
                .requires
                .iter()
                .map(|requirement| requirement.id.as_str()),
        )?;
        for requirement in &self.contract.requires {
            validate_capability_id(&requirement.id)?;
            if requirement
                .version
                .as_deref()
                .is_some_and(|version| version.trim().is_empty())
            {
                return Err(CapabilityValidationError::InvalidField(
                    "contract.requires.version must not be empty when set".into(),
                ));
            }
        }
        if self.contract.success_evidence.is_empty() {
            return Err(CapabilityValidationError::InvalidField(
                "contract.successEvidence must declare at least one claim".into(),
            ));
        }
        validate_unique_non_empty(
            "contract.effects",
            self.contract.effects.iter().map(|effect| effect.as_str()),
        )?;
        validate_permissions(&self.authority.permissions)?;
        for checkpoint in &self.authority.approvals {
            if checkpoint.before.trim().is_empty() {
                return Err(CapabilityValidationError::InvalidField(
                    "authority.approvals.before must not be empty".into(),
                ));
            }
        }
        if self.implementations.is_empty() {
            return Err(CapabilityValidationError::InvalidField(
                "implementations must contain at least one binding".into(),
            ));
        }
        validate_unique_non_empty(
            "implementations.id",
            self.implementations
                .iter()
                .map(|binding| binding.id.as_str()),
        )?;
        for binding in &self.implementations {
            if binding.reference.trim().is_empty() {
                return Err(CapabilityValidationError::InvalidField(format!(
                    "implementations.{}.reference must not be empty",
                    binding.id
                )));
            }
        }
        if let Some(provenance) = &self.provenance {
            if provenance.publisher.trim().is_empty()
                || !valid_digest(&provenance.package_digest)
                || provenance
                    .signature
                    .as_deref()
                    .is_some_and(|signature| signature.trim().is_empty())
            {
                return Err(CapabilityValidationError::InvalidField(
                    "provenance requires a publisher, sha256 package digest, and non-empty optional signature".into(),
                ));
            }
        }
        Ok(())
    }

    /// Computes the stable SHA-256 digest of this package's contract and origin.
    ///
    /// The digest and signature fields themselves are normalized out to avoid
    /// a circular digest. All other package content, including publisher and
    /// source, remains covered.
    ///
    /// # Errors
    ///
    /// Returns an error if the package cannot be serialized.
    pub fn content_digest(&self) -> Result<String, CapabilityValidationError> {
        let mut normalized = self.clone();
        if let Some(provenance) = &mut normalized.provenance {
            provenance.package_digest.clear();
            provenance.signature = None;
        }
        let bytes = serde_json::to_vec(&normalized).map_err(|_| {
            CapabilityValidationError::InvalidField(
                "capability package cannot be serialized for digesting".into(),
            )
        })?;
        let digest = Sha256::digest(bytes);
        Ok(format!("sha256:{digest:x}"))
    }

    /// Returns whether declared provenance matches the current package bytes.
    #[must_use]
    pub fn verify_content_digest(&self) -> bool {
        self.provenance
            .as_ref()
            .and_then(|provenance| {
                self.content_digest()
                    .ok()
                    .map(|digest| digest == provenance.package_digest)
            })
            .unwrap_or(false)
    }
}

fn valid_digest(digest: &str) -> bool {
    digest.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

/// Identity and human/computer-readable intent of a capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDefinition {
    /// Namespaced, dot-separated stable identifier, such as `billing.refund`.
    pub id: String,
    /// Publisher-assigned capability version.
    pub version: String,
    /// Concise outcome-oriented description.
    pub intent: String,
}

/// Input, output, effects, and evidence contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityContract {
    /// Capabilities that must be satisfied before this capability can run.
    #[serde(default)]
    pub requires: Vec<CapabilityRequirement>,
    /// JSON Schema object describing inputs.
    pub inputs: Value,
    /// JSON Schema object describing outputs.
    pub outputs: Value,
    /// Claims that must be supported by execution evidence before success.
    pub success_evidence: Vec<EvidenceClaim>,
    /// Declared real-world effects of execution.
    #[serde(default)]
    pub effects: Vec<EffectKind>,
    /// Retry and duplication safety expected by callers.
    #[serde(default)]
    pub idempotency: Idempotency,
    /// Required handling when execution becomes uncertain or fails.
    #[serde(default)]
    pub recovery: RecoveryStrategy,
}

/// Required child capability and optional exact version constraint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityRequirement {
    /// Namespaced capability identifier.
    pub id: String,
    /// Exact required version; omitted means the resolver may choose a version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// A claim the runtime must substantiate before reporting verified success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceClaim {
    /// Stable name of the claim, for example `payment_refunded`.
    pub id: String,
    /// Human-readable predicate the evidence must support.
    pub assertion: String,
    /// Evidence classes acceptable for this claim.
    pub accepted_types: Vec<EvidenceType>,
}

/// Classes of evidence understood by the portable contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceType {
    /// A receipt or query result supplied by the system of record.
    ProviderReceipt,
    /// An independently checked state predicate.
    StateAssertion,
    /// A signed attestation from an authorized party.
    SignedAttestation,
    /// Confirmation by an explicitly identified human.
    HumanAttestation,
}

/// Real-world effect categories used by policy engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    /// Reads state without intentional modification.
    ReadOnly,
    /// Creates or changes stored data.
    DataWrite,
    /// Communicates beyond the executing runtime.
    ExternalCommunication,
    /// Moves, charges, refunds, or commits money.
    Financial,
    /// Deletes or otherwise destroys data.
    Destructive,
    /// Cannot be reliably undone.
    Irreversible,
    /// Accesses a secret or delegated credential.
    CredentialAccess,
    /// Sends data to a network destination.
    NetworkEgress,
}

impl EffectKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::DataWrite => "data_write",
            Self::ExternalCommunication => "external_communication",
            Self::Financial => "financial",
            Self::Destructive => "destructive",
            Self::Irreversible => "irreversible",
            Self::CredentialAccess => "credential_access",
            Self::NetworkEgress => "network_egress",
        }
    }
}

/// Guarantees around repeated submissions of the same request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Idempotency {
    /// A caller-provided key is required to run safely.
    KeyRequired,
    /// Repeating the same input does not create additional effects.
    Guaranteed,
    /// Repeating execution may create additional effects.
    #[default]
    NotGuaranteed,
}

/// Required response to partial or failed execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryStrategy {
    /// Determine the external state before any further action.
    #[default]
    Reconcile,
    /// Safe bounded retry is supported.
    Retry,
    /// Continue from a recorded checkpoint.
    Resume,
    /// Invoke an explicit compensating action.
    Compensate,
    /// Stop and require an authorized human decision.
    HumanReview,
}

/// Portable authority requirements for a capability.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorityRequirements {
    /// Abstract permissions, resolved to provider scopes by a binding.
    #[serde(default)]
    pub permissions: Vec<String>,
    /// Approval gates that must be satisfied before named actions.
    #[serde(default)]
    pub approvals: Vec<ApprovalCheckpoint>,
    /// Machine-readable authority constraints, such as a maximum amount.
    #[serde(default)]
    pub constraints: BTreeMap<String, Value>,
}

/// A portable human or policy approval gate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalCheckpoint {
    /// Logical action that cannot begin before this gate is approved.
    pub before: String,
    /// Fields a compatible runtime should show to the approver.
    #[serde(default)]
    pub display: Vec<String>,
    /// Optional authorization role required from the approver.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_role: Option<String>,
}

/// A concrete technology binding for executing the portable contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImplementationKind {
    /// AgentMesh's learned workflow IR.
    AgentmeshWorkflow,
    /// Model Context Protocol tool composition.
    Mcp,
    /// OpenAPI/Arazzo workflow.
    Arazzo,
    /// Agent2Agent delegation.
    A2a,
    /// Local or remote command line execution.
    Cli,
    /// A human-operated implementation.
    HumanAssisted,
}

/// A named executable binding for a capability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImplementationBinding {
    /// Binding identifier, unique inside the package.
    pub id: String,
    /// Binding technology.
    pub kind: ImplementationKind,
    /// URI or package-relative reference to its executable definition.
    pub reference: String,
    /// Binding-specific configuration.
    #[serde(default)]
    pub configuration: BTreeMap<String, Value>,
}

/// Auditable package origin information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provenance {
    /// Identity of the publisher.
    pub publisher: String,
    /// Content digest of the signed package.
    pub package_digest: String,
    /// How this package originated.
    pub source: ProvenanceSource,
    /// Optional signature locator or key identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// Origin class of a capability package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceSource {
    /// Authored directly by a person or team.
    Authored,
    /// Extracted from a reviewed human demonstration.
    HumanDemonstration,
    /// Imported from another system.
    Imported,
}

/// Validation failure for a [`CapabilityPackage`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityValidationError {
    /// The package declares an unsupported schema revision.
    #[error("unsupported capability package schema version: {0}")]
    UnsupportedSchemaVersion(String),
    /// A field violates a portable contract invariant.
    #[error("invalid capability package: {0}")]
    InvalidField(String),
}

fn validate_capability_id(id: &str) -> Result<(), CapabilityValidationError> {
    let valid = id.split('.').count() >= 2
        && id.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        });
    if valid {
        Ok(())
    } else {
        Err(CapabilityValidationError::InvalidField(
            "capability.id must be lowercase, dot-separated, and namespaced".into(),
        ))
    }
}

fn validate_object_schema(name: &str, schema: &Value) -> Result<(), CapabilityValidationError> {
    if schema.is_object() {
        Ok(())
    } else {
        Err(CapabilityValidationError::InvalidField(format!(
            "{name} must be a JSON Schema object"
        )))
    }
}

fn validate_permissions(permissions: &[String]) -> Result<(), CapabilityValidationError> {
    validate_unique_non_empty(
        "authority.permissions",
        permissions.iter().map(String::as_str),
    )?;
    if permissions.iter().any(|permission| {
        permission.split('.').count() < 2
            || permission.split('.').any(|segment| {
                segment.is_empty()
                    || !segment.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                    })
            })
    }) {
        return Err(CapabilityValidationError::InvalidField(
            "authority.permissions must be dot-separated lowercase identifiers".into(),
        ));
    }
    Ok(())
}

fn validate_unique_non_empty<'a>(
    name: &str,
    values: impl Iterator<Item = &'a str>,
) -> Result<(), CapabilityValidationError> {
    let mut seen = BTreeSet::new();
    for value in values {
        if value.trim().is_empty() || !seen.insert(value) {
            return Err(CapabilityValidationError::InvalidField(format!(
                "{name} must contain unique, non-empty values"
            )));
        }
    }
    Ok(())
}
