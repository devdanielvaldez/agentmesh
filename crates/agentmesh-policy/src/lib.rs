//! Compiled contextual policy evaluation and bounded human approvals.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Mutex, MutexGuard},
};

use agentmesh_authn::{AuthenticatedPrincipal, AuthenticationStrength};
use agentmesh_authz::{Action, Resource};
use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_registry::RegistryScope;
use serde::{Deserialize, Serialize};
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
