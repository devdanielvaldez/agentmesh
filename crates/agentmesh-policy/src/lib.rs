//! Compiled contextual policy evaluation and bounded human approvals.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Mutex, MutexGuard},
};

use agentmesh_authn::{AuthenticatedPrincipal, AuthenticationStrength};
use agentmesh_authz::{Action, Resource};
use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_protocol::{CapabilityPackage, EffectKind};
use agentmesh_registry::RegistryScope;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Maximum rules in one compiled bundle.
pub const MAX_POLICY_RULES: usize = 10_000;
/// Maximum approval records in the local workflow.
pub const MAX_APPROVALS: usize = 100_000;

/// Ordered capability risk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Read-only or negligible impact.
    Low,
    /// Bounded reversible change.
    Medium,
    /// Sensitive or materially mutating action.
    High,
    /// Destructive or privileged action.
    Critical,
}

/// Derives a conservative policy risk from declared capability effects.
///
/// An undeclared effect set is critical: a runtime must not assume an unknown
/// capability is read-only.
pub fn capability_risk(package: &CapabilityPackage) -> Risk {
    let effects = &package.contract.effects;
    if effects.is_empty()
        || effects
            .iter()
            .any(|effect| matches!(effect, EffectKind::Destructive | EffectKind::Irreversible))
    {
        Risk::Critical
    } else if effects.iter().any(|effect| {
        matches!(
            effect,
            EffectKind::Financial | EffectKind::CredentialAccess | EffectKind::NetworkEgress
        )
    }) {
        Risk::High
    } else if effects.iter().any(|effect| {
        matches!(
            effect,
            EffectKind::DataWrite | EffectKind::ExternalCommunication
        )
    }) {
        Risk::Medium
    } else {
        Risk::Low
    }
}

/// Static admission policy for a portable capability package.
///
/// This check runs before implementation selection or tool execution. It is
/// separate from request-time RBAC, which is evaluated later by
/// [`PolicyBundle`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityAdmissionPolicy {
    /// Effects permitted in this deployment.
    pub allowed_effects: BTreeSet<EffectKind>,
    /// Portable permissions delegated to this deployment.
    pub granted_permissions: BTreeSet<String>,
    /// Highest admitted effect-derived risk.
    pub maximum_risk: Risk,
    /// Risk level at which an approval is always required.
    pub approval_at_or_above: Risk,
    /// Whether packages without provenance must be rejected.
    pub require_provenance: bool,
}

impl Default for CapabilityAdmissionPolicy {
    fn default() -> Self {
        Self {
            allowed_effects: BTreeSet::from([EffectKind::ReadOnly]),
            granted_permissions: BTreeSet::new(),
            maximum_risk: Risk::Low,
            approval_at_or_above: Risk::Medium,
            require_provenance: true,
        }
    }
}

/// Result of pre-execution capability admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityAdmissionDecision {
    /// Whether execution may proceed, be denied, or await approval.
    pub effect: PolicyEffect,
    /// Derived capability risk used to make the decision.
    pub risk: Risk,
    /// Stable machine-readable explanation.
    pub reason: String,
}

/// External cryptographic verifier for package publisher signatures.
///
/// Implementations own key discovery, key rotation, revocation, and the chosen
/// signature algorithm. The verifier receives the already recomputed content
/// digest so signature checks bind the exact loaded package.
pub trait PackageSignatureVerifier: Send + Sync {
    /// Verifies a signature made by `publisher` over `content_digest`.
    fn verify(&self, publisher: &str, content_digest: &str, signature: &str) -> bool;
}

/// Publisher admission rules applied after package integrity validation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilityTrustPolicy {
    /// Exact approved publisher IDs; empty means any publisher can be evaluated.
    pub trusted_publishers: BTreeSet<String>,
    /// Whether every admitted package must carry a verifiable signature.
    pub require_signature: bool,
}

/// Result of package publisher trust evaluation, separate from execution policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityTrustDecision {
    /// Whether provenance and publisher signature satisfy trust policy.
    pub trusted: bool,
    /// Stable machine-readable explanation.
    pub reason: String,
}

/// Verifies publisher identity, package integrity, and any declared signature.
pub fn verify_capability_trust(
    package: &CapabilityPackage,
    policy: &CapabilityTrustPolicy,
    verifier: &dyn PackageSignatureVerifier,
) -> CapabilityTrustDecision {
    let Some(provenance) = &package.provenance else {
        return CapabilityTrustDecision {
            trusted: false,
            reason: "provenance_required".into(),
        };
    };
    if package.validate().is_err() || !package.verify_content_digest() {
        return CapabilityTrustDecision {
            trusted: false,
            reason: "package_digest_mismatch".into(),
        };
    }
    if !policy.trusted_publishers.is_empty()
        && !policy.trusted_publishers.contains(&provenance.publisher)
    {
        return CapabilityTrustDecision {
            trusted: false,
            reason: "publisher_not_trusted".into(),
        };
    }
    match provenance.signature.as_deref() {
        Some(signature)
            if verifier.verify(&provenance.publisher, &provenance.package_digest, signature) =>
        {
            CapabilityTrustDecision {
                trusted: true,
                reason: "publisher_signature_verified".into(),
            }
        }
        Some(_) => CapabilityTrustDecision {
            trusted: false,
            reason: "publisher_signature_invalid".into(),
        },
        None if policy.require_signature => CapabilityTrustDecision {
            trusted: false,
            reason: "publisher_signature_required".into(),
        },
        None => CapabilityTrustDecision {
            trusted: true,
            reason: "publisher_signature_not_required".into(),
        },
    }
}

/// Evaluates a capability declaration before any provider is invoked.
pub fn admit_capability(
    package: &CapabilityPackage,
    policy: &CapabilityAdmissionPolicy,
) -> CapabilityAdmissionDecision {
    let risk = capability_risk(package);
    if package.validate().is_err() {
        return CapabilityAdmissionDecision {
            effect: PolicyEffect::Deny,
            risk,
            reason: "invalid_capability_package".into(),
        };
    }
    if policy.require_provenance && package.provenance.is_none() {
        return CapabilityAdmissionDecision {
            effect: PolicyEffect::Deny,
            risk,
            reason: "provenance_required".into(),
        };
    }
    if package.provenance.is_some() && !package.verify_content_digest() {
        return CapabilityAdmissionDecision {
            effect: PolicyEffect::Deny,
            risk,
            reason: "package_digest_mismatch".into(),
        };
    }
    if package
        .contract
        .effects
        .iter()
        .any(|effect| !policy.allowed_effects.contains(effect))
    {
        return CapabilityAdmissionDecision {
            effect: PolicyEffect::Deny,
            risk,
            reason: "effect_not_allowed".into(),
        };
    }
    if package
        .authority
        .permissions
        .iter()
        .any(|permission| !policy.granted_permissions.contains(permission))
    {
        return CapabilityAdmissionDecision {
            effect: PolicyEffect::Deny,
            risk,
            reason: "permission_not_granted".into(),
        };
    }
    if risk > policy.maximum_risk {
        return CapabilityAdmissionDecision {
            effect: PolicyEffect::Deny,
            risk,
            reason: "risk_exceeds_policy".into(),
        };
    }
    if !package.authority.approvals.is_empty() || risk >= policy.approval_at_or_above {
        return CapabilityAdmissionDecision {
            effect: PolicyEffect::RequireApproval,
            risk,
            reason: "capability_approval_required".into(),
        };
    }
    CapabilityAdmissionDecision {
        effect: PolicyEffect::Allow,
        risk,
        reason: "capability_admitted".into(),
    }
}

/// Policy outcome beyond RBAC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEffect {
    /// Continue with declared obligations.
    Allow,
    /// Reject the operation.
    Deny,
    /// Pause until a separate approver resolves it.
    RequireApproval,
}

/// Exact, bounded match dimensions compiled for the hot path.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyMatch {
    /// Optional action set; empty matches all actions.
    #[serde(default)]
    pub actions: BTreeSet<Action>,
    /// Required authenticated roles.
    #[serde(default)]
    pub roles: BTreeSet<String>,
    /// Exact deployment environment.
    pub environment: Option<String>,
    /// Exact data classification.
    pub data_classification: Option<String>,
    /// Inclusive minimum risk.
    pub minimum_risk: Option<Risk>,
    /// Inclusive maximum risk.
    pub maximum_risk: Option<Risk>,
    /// Minimum authentication strength.
    pub minimum_authentication: Option<AuthenticationStrength>,
    /// UTC minute-of-day start, inclusive.
    pub start_minute_utc: Option<u16>,
    /// UTC minute-of-day end, exclusive.
    pub end_minute_utc: Option<u16>,
    /// Exact safe argument metadata; raw arguments are never stored.
    #[serde(default)]
    pub argument_metadata: BTreeMap<String, String>,
}

/// One compiled contextual policy rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyRule {
    /// Stable rule ID.
    pub id: String,
    /// Higher values take precedence.
    pub priority: u32,
    /// Context match.
    #[serde(rename = "match")]
    pub matcher: PolicyMatch,
    /// Decision when matched.
    pub effect: PolicyEffect,
    /// Stable reason code.
    pub reason: String,
    /// Bounded obligations such as `redact_output` or `audit_required`.
    #[serde(default)]
    pub obligations: BTreeSet<String>,
}

/// Immutable evaluation input.
pub struct PolicyContext<'a> {
    /// Authenticated caller.
    pub principal: &'a AuthenticatedPrincipal,
    /// Tenant scope.
    pub scope: &'a RegistryScope,
    /// Authorized action.
    pub action: &'a Action,
    /// Target resource.
    pub resource: &'a Resource,
    /// Deployment environment.
    pub environment: &'a str,
    /// Data classification.
    pub data_classification: &'a str,
    /// Capability/operation risk.
    pub risk: Risk,
    /// UTC minute of day supplied by the request context.
    pub minute_utc: u16,
    /// Sanitized argument facts, never raw arguments.
    pub argument_metadata: &'a BTreeMap<String, String>,
}

/// Explainable policy result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyDecision {
    /// Allow, deny, or request approval.
    pub effect: PolicyEffect,
    /// Matched rule, absent for default deny.
    pub matched_rule: Option<String>,
    /// Stable reason code.
    pub reason: String,
    /// Obligations enforced by later pipeline stages.
    pub obligations: BTreeSet<String>,
    /// Bundle revision used.
    pub revision: u64,
}

/// Validated immutable rule bundle.
#[derive(Debug, Clone)]
pub struct PolicyBundle {
    organization: String,
    revision: u64,
    rules: Vec<PolicyRule>,
}

impl PolicyBundle {
    /// Compiles deterministic rule precedence and rejects ambiguity.
    ///
    /// # Errors
    ///
    /// Rejects invalid, duplicate, ambiguous, or unbounded policies.
    pub fn compile(
        organization: impl Into<String>,
        revision: u64,
        mut rules: Vec<PolicyRule>,
    ) -> Result<Self, AgentMeshError> {
        let organization = organization.into();
        validate_text(&organization)?;
        if rules.len() > MAX_POLICY_RULES {
            return Err(configuration(
                "The policy bundle exceeds its hard rule limit.",
            ));
        }
        let mut ids = BTreeSet::new();
        let mut matches = BTreeMap::new();
        for rule in &rules {
            validate_rule(rule)?;
            if !ids.insert(rule.id.as_str()) {
                return Err(configuration("Policy rule identifiers must be unique."));
            }
            if let Some(effect) = matches.insert((rule.priority, rule.matcher.clone()), rule.effect)
            {
                if effect != rule.effect {
                    return Err(configuration(
                        "Equal-precedence policies cannot produce conflicting effects.",
                    ));
                }
            }
        }
        rules.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(Self {
            organization,
            revision,
            rules,
        })
    }

    /// Evaluates without I/O or mutable global state.
    pub fn evaluate(&self, context: &PolicyContext<'_>) -> PolicyDecision {
        if context.scope.organization() != self.organization
            || &context.principal.scope != context.scope
        {
            return default_deny(self.revision, "tenant_mismatch");
        }
        self.rules
            .iter()
            .find(|rule| policy_matches(&rule.matcher, context))
            .map_or_else(
                || default_deny(self.revision, "no_policy_match"),
                |rule| PolicyDecision {
                    effect: rule.effect,
                    matched_rule: Some(rule.id.clone()),
                    reason: rule.reason.clone(),
                    obligations: rule.obligations.clone(),
                    revision: self.revision,
                },
            )
    }

    /// Evaluates a bounded batch for dry-run and rollout simulation.
    ///
    /// # Errors
    ///
    /// Rejects batches larger than 10,000 contexts.
    pub fn simulate(
        &self,
        contexts: &[PolicyContext<'_>],
    ) -> Result<Vec<PolicyDecision>, AgentMeshError> {
        if contexts.len() > 10_000 {
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "The policy simulation batch is too large.",
            ));
        }
        Ok(contexts
            .iter()
            .map(|context| self.evaluate(context))
            .collect())
    }
}

/// Opaque approval request identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ApprovalId(Uuid);

/// Approval lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalState {
    /// Waiting for an independent reviewer.
    Pending,
    /// Approved by a different principal.
    Approved,
    /// Explicitly denied.
    Denied,
}

/// Sanitized approval record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRecord {
    /// Opaque ID.
    pub id: ApprovalId,
    /// Tenant scope.
    pub scope: RegistryScope,
    /// Requesting principal.
    pub requester: String,
    /// Stable action/resource fingerprint supplied by the policy adapter.
    pub request_fingerprint: String,
    /// Current state.
    pub state: ApprovalState,
    /// Expiration timestamp.
    pub expires_at_millis: u64,
    /// Resolver, once decided.
    pub resolved_by: Option<String>,
}

/// Approval request bound to one capability action and its displayed values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityApprovalRequest {
    /// Persistent approval lifecycle record.
    pub record: ApprovalRecord,
    /// Capability package identity being approved.
    pub capability_id: String,
    /// Capability version being approved.
    pub capability_version: String,
    /// Action gate the approval authorizes.
    pub checkpoint: String,
    /// Exact safe fields shown to the approver.
    pub display_values: BTreeMap<String, String>,
}

/// Bounded local approval workflow used by standalone deployments.
#[derive(Default)]
pub struct ApprovalStore {
    records: Mutex<BTreeMap<(RegistryScope, ApprovalId), ApprovalRecord>>,
}

impl ApprovalStore {
    /// Creates an approval request.
    ///
    /// # Errors
    ///
    /// Rejects invalid values, expired requests, and store exhaustion.
    pub fn request(
        &self,
        scope: RegistryScope,
        requester: impl Into<String>,
        fingerprint: impl Into<String>,
        expires_at_millis: u64,
        now_millis: u64,
    ) -> Result<ApprovalRecord, AgentMeshError> {
        let requester = requester.into();
        let request_fingerprint = fingerprint.into();
        validate_text(&requester)?;
        validate_text(&request_fingerprint)?;
        if expires_at_millis <= now_millis {
            return Err(configuration("Approval expiration must be in the future."));
        }
        let mut records = lock(&self.records);
        if records.len() >= MAX_APPROVALS {
            return Err(AgentMeshError::new(
                ErrorCode::StorageUnavailable,
                "The approval store is at capacity.",
            ));
        }
        let record = ApprovalRecord {
            id: ApprovalId(Uuid::new_v4()),
            scope: scope.clone(),
            requester,
            request_fingerprint,
            state: ApprovalState::Pending,
            expires_at_millis,
            resolved_by: None,
        };
        records.insert((scope, record.id), record.clone());
        Ok(record)
    }

    /// Creates a checkpoint approval tied to the exact values shown to a human.
    ///
    /// The returned record stores only a digest of the request context. The
    /// display values are returned to the caller for rendering and are never
    /// persisted by this store.
    ///
    /// # Errors
    ///
    /// Rejects invalid packages, unknown checkpoints, missing displayed values,
    /// invalid context, expired requests, or a full approval store.
    pub fn request_capability_checkpoint(
        &self,
        scope: RegistryScope,
        requester: impl Into<String>,
        package: &CapabilityPackage,
        checkpoint: &str,
        action_values: &BTreeMap<String, Value>,
        expires_at_millis: u64,
        now_millis: u64,
    ) -> Result<CapabilityApprovalRequest, AgentMeshError> {
        package
            .validate()
            .map_err(|_| configuration("The capability package is invalid for approval."))?;
        let gate = package
            .authority
            .approvals
            .iter()
            .find(|gate| gate.before == checkpoint)
            .ok_or_else(|| configuration("The capability approval checkpoint is unknown."))?;
        let mut display_values = BTreeMap::new();
        for field in &gate.display {
            let value = action_values.get(field).ok_or_else(|| {
                configuration("An approval display field is missing from the action.")
            })?;
            validate_text(field)?;
            let rendered = render_approval_value(value);
            if rendered.len() > 4_096 || rendered.chars().any(char::is_control) {
                return Err(configuration(
                    "An approval display value is invalid or unbounded.",
                ));
            }
            display_values.insert(field.clone(), rendered);
        }
        let fingerprint = approval_fingerprint(
            &package.capability.id,
            &package.capability.version,
            checkpoint,
            &display_values,
            action_values,
        )?;
        let record = self.request(scope, requester, fingerprint, expires_at_millis, now_millis)?;
        Ok(CapabilityApprovalRequest {
            record,
            capability_id: package.capability.id.clone(),
            capability_version: package.capability.version.clone(),
            checkpoint: checkpoint.into(),
            display_values,
        })
    }

    /// Resolves a pending request using separation of duties.
    ///
    /// # Errors
    ///
    /// Rejects missing, expired, already-resolved, self-approved, or cross-tenant requests.
    pub fn resolve(
        &self,
        scope: &RegistryScope,
        id: ApprovalId,
        approver: &str,
        grant: bool,
        now_millis: u64,
    ) -> Result<ApprovalRecord, AgentMeshError> {
        validate_text(approver)?;
        let mut records = lock(&self.records);
        let record = records.get_mut(&(scope.clone(), id)).ok_or_else(|| {
            AgentMeshError::new(
                ErrorCode::ApprovalRequired,
                "The approval request is unavailable.",
            )
        })?;
        if record.state != ApprovalState::Pending
            || record.expires_at_millis <= now_millis
            || record.requester == approver
        {
            return Err(AgentMeshError::new(
                ErrorCode::Conflict,
                "The approval request cannot be resolved.",
            ));
        }
        record.state = if grant {
            ApprovalState::Approved
        } else {
            ApprovalState::Denied
        };
        record.resolved_by = Some(approver.into());
        Ok(record.clone())
    }
}

/// Checks whether an approval request still describes the current action.
///
/// Any change to capability identity, checkpoint, or displayed values requires
/// a fresh approval.
pub fn capability_approval_matches(
    request: &CapabilityApprovalRequest,
    package: &CapabilityPackage,
    checkpoint: &str,
    action_values: &BTreeMap<String, Value>,
) -> bool {
    if package.capability.id != request.capability_id
        || package.capability.version != request.capability_version
        || checkpoint != request.checkpoint
        || package.validate().is_err()
    {
        return false;
    }
    let Ok(current) = displayed_values(package, checkpoint, action_values) else {
        return false;
    };
    let Ok(fingerprint) = approval_fingerprint(
        &package.capability.id,
        &package.capability.version,
        checkpoint,
        &current,
        action_values,
    ) else {
        return false;
    };
    request.record.request_fingerprint == fingerprint
}

fn displayed_values(
    package: &CapabilityPackage,
    checkpoint: &str,
    action_values: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, String>, AgentMeshError> {
    let gate = package
        .authority
        .approvals
        .iter()
        .find(|gate| gate.before == checkpoint)
        .ok_or_else(|| configuration("The capability approval checkpoint is unknown."))?;
    let mut display_values = BTreeMap::new();
    for field in &gate.display {
        let value = action_values.get(field).ok_or_else(|| {
            configuration("An approval display field is missing from the action.")
        })?;
        validate_text(field)?;
        let rendered = render_approval_value(value);
        if rendered.len() > 4_096 || rendered.chars().any(char::is_control) {
            return Err(configuration(
                "An approval display value is invalid or unbounded.",
            ));
        }
        display_values.insert(field.clone(), rendered);
    }
    Ok(display_values)
}

fn render_approval_value(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), ToOwned::to_owned)
}

fn approval_fingerprint(
    capability_id: &str,
    capability_version: &str,
    checkpoint: &str,
    display_values: &BTreeMap<String, String>,
    action_values: &BTreeMap<String, Value>,
) -> Result<String, AgentMeshError> {
    let payload = serde_json::to_vec(&(
        capability_id,
        capability_version,
        checkpoint,
        display_values,
        action_values,
    ))
    .map_err(|_| configuration("The capability approval context could not be encoded."))?;
    let digest = Sha256::digest(payload);
    Ok(format!("sha256:{digest:x}"))
}

fn policy_matches(matcher: &PolicyMatch, context: &PolicyContext<'_>) -> bool {
    (matcher.actions.is_empty() || matcher.actions.contains(context.action))
        && matcher
            .roles
            .iter()
            .all(|role| context.principal.asserted_roles.contains(role))
        && matcher
            .environment
            .as_deref()
            .is_none_or(|value| value == context.environment)
        && matcher
            .data_classification
            .as_deref()
            .is_none_or(|value| value == context.data_classification)
        && matcher
            .minimum_risk
            .is_none_or(|value| context.risk >= value)
        && matcher
            .maximum_risk
            .is_none_or(|value| context.risk <= value)
        && matcher
            .minimum_authentication
            .is_none_or(|value| context.principal.strength >= value)
        && time_matches(
            matcher.start_minute_utc,
            matcher.end_minute_utc,
            context.minute_utc,
        )
        && matcher
            .argument_metadata
            .iter()
            .all(|(key, value)| context.argument_metadata.get(key) == Some(value))
}

fn time_matches(start: Option<u16>, end: Option<u16>, minute: u16) -> bool {
    match (start, end) {
        (None, None) => true,
        (Some(start), Some(end)) if start <= end => minute >= start && minute < end,
        (Some(start), Some(end)) => minute >= start || minute < end,
        _ => false,
    }
}

fn validate_rule(rule: &PolicyRule) -> Result<(), AgentMeshError> {
    validate_text(&rule.id)?;
    validate_text(&rule.reason)?;
    if rule.matcher.start_minute_utc.is_some() != rule.matcher.end_minute_utc.is_some()
        || rule
            .matcher
            .start_minute_utc
            .is_some_and(|value| value >= 1_440)
        || rule
            .matcher
            .end_minute_utc
            .is_some_and(|value| value > 1_440)
        || rule
            .matcher
            .minimum_risk
            .zip(rule.matcher.maximum_risk)
            .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        return Err(configuration("Policy ranges and time windows are invalid."));
    }
    for value in rule.matcher.roles.iter().chain(&rule.obligations) {
        validate_text(value)?;
    }
    for value in [
        rule.matcher.environment.as_deref(),
        rule.matcher.data_classification.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        validate_text(value)?;
    }
    for (key, value) in &rule.matcher.argument_metadata {
        validate_text(key)?;
        validate_text(value)?;
    }
    Ok(())
}

fn default_deny(revision: u64, reason: &str) -> PolicyDecision {
    PolicyDecision {
        effect: PolicyEffect::Deny,
        matched_rule: None,
        reason: reason.into(),
        obligations: BTreeSet::new(),
        revision,
    }
}

fn validate_text(value: &str) -> Result<(), AgentMeshError> {
    if value.trim().is_empty() || value.len() > 1_024 || value.chars().any(char::is_control) {
        return Err(configuration("A policy value is invalid or unbounded."));
    }
    Ok(())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn configuration(message: &'static str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::ConfigurationInvalid, message)
}
