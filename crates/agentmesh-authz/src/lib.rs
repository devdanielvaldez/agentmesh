//! Immutable, revision-aware, explainable RBAC authorization.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};

use agentmesh_authn::AuthenticatedPrincipal;
use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_registry::RegistryScope;
use serde::{Deserialize, Serialize};

/// Maximum rules, roles, and bindings in one snapshot.
pub const MAX_AUTHORIZATION_ITEMS: usize = 10_000;
/// Maximum cached decisions.
pub const MAX_DECISION_CACHE_ENTRIES: usize = 100_000;

/// Stable authorization actions.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Discover or list a capability.
    Discover,
    /// Invoke a tool.
    ToolCall,
    /// Read a resource.
    ResourceRead,
    /// Subscribe to a resource.
    ResourceSubscribe,
    /// Read task state or result.
    TaskRead,
    /// Cancel a task.
    TaskCancel,
    /// Perform an administrative mutation.
    Admin,
    /// Forward-compatible application action.
    Custom(String),
}

/// Normalized authorization target.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Resource {
    /// Stable resource family such as `tool` or `task`.
    pub kind: String,
    /// Stable name, URI, or identifier.
    pub name: String,
    /// Trusted resource metadata.
    pub labels: BTreeMap<String, String>,
}

/// Explicit allow or deny rule effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// Grants access when no deny matches.
    Allow,
    /// Rejects access and always wins.
    Deny,
}

/// Permission attached to one or more roles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionRule {
    /// Stable rule identifier.
    pub id: String,
    /// Allow or explicit deny.
    pub effect: Effect,
    /// Actions matched by this rule.
    pub actions: BTreeSet<Action>,
    /// Exact kind or `*`.
    pub resource_kind: String,
    /// Exact name or `*`.
    pub resource_name: String,
    /// Labels required on the target.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// Named collection of permission rule IDs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Role {
    /// Stable role name.
    pub name: String,
    /// Referenced permission IDs.
    pub permissions: BTreeSet<String>,
}

/// Assigns a role to principals at organization or namespace scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBinding {
    /// Stable binding identifier.
    pub id: String,
    /// Existing role name.
    pub role: String,
    /// Principal IDs receiving the role.
    pub principals: BTreeSet<String>,
    /// `None` inherits across namespaces in the snapshot organization.
    pub namespace: Option<String>,
    /// Optional delegated-user allowlist. Empty rejects delegation through this binding.
    #[serde(default)]
    pub delegated_users: BTreeSet<String>,
}

/// Version identifier invalidating cached decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuthorizationRevision(pub u64);

/// Immutable compiled RBAC data for one organization.
#[derive(Debug, Clone)]
pub struct AuthorizationSnapshot {
    organization: String,
    revision: AuthorizationRevision,
    permissions: BTreeMap<String, PermissionRule>,
    roles: BTreeMap<String, Role>,
    bindings: Vec<RoleBinding>,
}

impl AuthorizationSnapshot {
    /// Validates and compiles one RBAC revision.
    ///
    /// # Errors
    ///
    /// Rejects unbounded collections, duplicate IDs, invalid values, and dangling references.
    pub fn compile(
        organization: impl Into<String>,
        revision: AuthorizationRevision,
        permissions: Vec<PermissionRule>,
        roles: Vec<Role>,
        bindings: Vec<RoleBinding>,
    ) -> Result<Self, AgentMeshError> {
        let organization = organization.into();
        validate_text(&organization)?;
        if permissions.len() > MAX_AUTHORIZATION_ITEMS
            || roles.len() > MAX_AUTHORIZATION_ITEMS
            || bindings.len() > MAX_AUTHORIZATION_ITEMS
        {
            return Err(configuration(
                "The authorization snapshot exceeds its hard limits.",
            ));
        }
        let mut permission_map = BTreeMap::new();
        for permission in permissions {
            validate_permission(&permission)?;
            if permission_map
                .insert(permission.id.clone(), permission)
                .is_some()
            {
                return Err(configuration("Permission identifiers must be unique."));
            }
        }
        let mut role_map = BTreeMap::new();
        for role in roles {
            validate_text(&role.name)?;
            if role.permissions.is_empty()
                || role
                    .permissions
                    .iter()
                    .any(|id| !permission_map.contains_key(id))
            {
                return Err(configuration("Roles must reference existing permissions."));
            }
            if role_map.insert(role.name.clone(), role).is_some() {
                return Err(configuration("Role names must be unique."));
            }
        }
        let mut binding_ids = BTreeSet::new();
        for binding in &bindings {
            validate_text(&binding.id)?;
            if !binding_ids.insert(binding.id.as_str())
                || !role_map.contains_key(&binding.role)
                || binding.principals.is_empty()
            {
                return Err(configuration(
                    "Role bindings are duplicate, empty, or reference an unknown role.",
                ));
            }
            for value in binding.principals.iter().chain(&binding.delegated_users) {
                validate_text(value)?;
            }
            if let Some(namespace) = &binding.namespace {
                validate_text(namespace)?;
            }
        }
        Ok(Self {
            organization,
            revision,
            permissions: permission_map,
            roles: role_map,
            bindings,
        })
    }

    /// Compiled revision.
    pub const fn revision(&self) -> AuthorizationRevision {
        self.revision
    }
}

/// One authorization query.
pub struct AuthorizationRequest<'a> {
    /// Authenticated caller.
    pub principal: &'a AuthenticatedPrincipal,
    /// Tenant boundary containing the target.
    pub scope: &'a RegistryScope,
    /// Requested action.
    pub action: &'a Action,
    /// Requested target.
    pub resource: &'a Resource,
    /// Delegated end user, when present.
    pub delegated_user: Option<&'a str>,
}

/// Explainable authorization result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationDecision {
    /// Whether access is granted.
    pub allowed: bool,
    /// Stable matched permission ID.
    pub matched_rule: Option<String>,
    /// Low-cardinality explanation code.
    pub reason: &'static str,
    /// Revision used for the decision.
    pub revision: AuthorizationRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CacheKey {
    revision: AuthorizationRevision,
    principal: String,
    organization: String,
    namespace: String,
    action: Action,
    resource: Resource,
    delegated_user: Option<String>,
}

/// Compiled authorization engine with a bounded revision-aware cache.
pub struct AuthorizationEngine {
    snapshot: AuthorizationSnapshot,
    cache_capacity: usize,
    cache: Mutex<BTreeMap<CacheKey, AuthorizationDecision>>,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl AuthorizationEngine {
    /// Creates an engine with explicit bounded cache capacity.
    ///
    /// # Errors
    ///
    /// Rejects a cache capacity above the hard limit.
    pub fn new(
        snapshot: AuthorizationSnapshot,
        cache_capacity: usize,
    ) -> Result<Self, AgentMeshError> {
        if cache_capacity > MAX_DECISION_CACHE_ENTRIES {
            return Err(configuration(
                "The authorization decision cache exceeds its hard limit.",
            ));
        }
        Ok(Self {
            snapshot,
            cache_capacity,
            cache: Mutex::new(BTreeMap::new()),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        })
    }

    /// Evaluates RBAC with deny precedence and tenant isolation.
    pub fn evaluate(&self, request: &AuthorizationRequest<'_>) -> AuthorizationDecision {
        let key = CacheKey {
            revision: self.snapshot.revision,
            principal: request.principal.id.clone(),
            organization: request.scope.organization().into(),
            namespace: request.scope.namespace().into(),
            action: request.action.clone(),
            resource: request.resource.clone(),
            delegated_user: request.delegated_user.map(str::to_owned),
        };
        if let Some(decision) = lock(&self.cache).get(&key).cloned() {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return decision;
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        let decision = self.evaluate_uncached(request);
        if self.cache_capacity > 0 {
            let mut cache = lock(&self.cache);
            if cache.len() == self.cache_capacity {
                if let Some(oldest) = cache.keys().next().cloned() {
                    cache.remove(&oldest);
                }
            }
            cache.insert(key, decision.clone());
        }
        decision
    }

    /// Returns cache hits and misses for low-cardinality metrics.
    pub fn cache_metrics(&self) -> (u64, u64) {
        (
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed),
        )
    }

    /// Filters resources using the exact same decision path as invocation.
    pub fn visible<'a>(
        &self,
        principal: &'a AuthenticatedPrincipal,
        scope: &'a RegistryScope,
        action: &'a Action,
        resources: &'a [Resource],
    ) -> Vec<&'a Resource> {
        resources
            .iter()
            .filter(|resource| {
                self.evaluate(&AuthorizationRequest {
                    principal,
                    scope,
                    action,
                    resource,
                    delegated_user: None,
                })
                .allowed
            })
            .collect()
    }

    fn evaluate_uncached(&self, request: &AuthorizationRequest<'_>) -> AuthorizationDecision {
        let deny = |reason| AuthorizationDecision {
            allowed: false,
            matched_rule: None,
            reason,
            revision: self.snapshot.revision,
        };
        if request.scope.organization() != self.snapshot.organization
            || &request.principal.scope != request.scope
        {
            return deny("tenant_mismatch");
        }
        let mut matched_allow = None;
        for binding in &self.snapshot.bindings {
            let asserted = request.principal.asserted_roles.contains(&binding.role);
            if !asserted && !binding.principals.contains(&request.principal.id) {
                continue;
            }
            if binding
                .namespace
                .as_deref()
                .is_some_and(|namespace| namespace != request.scope.namespace())
            {
                continue;
            }
            if request
                .delegated_user
                .is_some_and(|user| !binding.delegated_users.contains(user))
            {
                continue;
            }
            let Some(role) = self.snapshot.roles.get(&binding.role) else {
                continue;
            };
            for permission_id in &role.permissions {
                let Some(permission) = self.snapshot.permissions.get(permission_id) else {
                    continue;
                };
                if permission_matches(permission, request.action, request.resource) {
                    if permission.effect == Effect::Deny {
                        return AuthorizationDecision {
                            allowed: false,
                            matched_rule: Some(permission.id.clone()),
                            reason: "explicit_deny",
                            revision: self.snapshot.revision,
                        };
                    }
                    matched_allow.get_or_insert_with(|| permission.id.clone());
                }
            }
        }
        matched_allow.map_or_else(
            || deny("no_allow_rule"),
            |rule| AuthorizationDecision {
                allowed: true,
                matched_rule: Some(rule),
                reason: "explicit_allow",
                revision: self.snapshot.revision,
            },
        )
    }
}

fn permission_matches(permission: &PermissionRule, action: &Action, resource: &Resource) -> bool {
    permission.actions.contains(action)
        && (permission.resource_kind == "*" || permission.resource_kind == resource.kind)
        && (permission.resource_name == "*" || permission.resource_name == resource.name)
        && permission
            .labels
            .iter()
            .all(|(key, value)| resource.labels.get(key) == Some(value))
}

fn validate_permission(permission: &PermissionRule) -> Result<(), AgentMeshError> {
    validate_text(&permission.id)?;
    validate_text(&permission.resource_kind)?;
    validate_text(&permission.resource_name)?;
    if permission.actions.is_empty() {
        return Err(configuration("Permission action sets cannot be empty."));
    }
    for action in &permission.actions {
        if let Action::Custom(value) = action {
            validate_text(value)?;
        }
    }
    for (key, value) in &permission.labels {
        validate_text(key)?;
        validate_text(value)?;
    }
    Ok(())
}

fn validate_text(value: &str) -> Result<(), AgentMeshError> {
    if value.trim().is_empty() || value.len() > 1_024 || value.chars().any(char::is_control) {
        return Err(configuration(
            "An authorization value is invalid or unbounded.",
        ));
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
