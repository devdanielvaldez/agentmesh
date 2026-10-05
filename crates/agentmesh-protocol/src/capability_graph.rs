//! Deterministic dependency resolution for composed capabilities.

use std::collections::{BTreeMap, BTreeSet};

use crate::{CapabilityPackage, ImplementationBinding, ImplementationKind};

/// Request sent to another agent to discover a usable capability offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityNegotiationRequest {
    /// Exact capability identity requested by the caller.
    pub capability_id: String,
    /// Optional exact version; without it, only a single catalog version is acceptable.
    pub version: Option<String>,
    /// Effects the requesting agent permits for this task.
    pub accepted_effects: BTreeSet<crate::EffectKind>,
    /// Portable permissions already delegated to the remote agent.
    pub granted_permissions: BTreeSet<String>,
    /// Implementations currently available at the remote agent.
    pub available_bindings: BTreeSet<String>,
    /// Runtime preference order for implementation kinds.
    pub preferred_kinds: Vec<ImplementationKind>,
}

/// Concrete offer returned by a remote capability provider.
#[derive(Debug, Clone, PartialEq)]
pub struct CapabilityOffer {
    /// Exact capability identity.
    pub capability_id: String,
    /// Exact capability version.
    pub capability_version: String,
    /// Selected implementation binding.
    pub implementation: ImplementationBinding,
    /// Declared effects of the capability.
    pub effects: Vec<crate::EffectKind>,
    /// Permissions the provider will exercise.
    pub requested_permissions: Vec<String>,
    /// Input contract schema.
    pub input_schema: serde_json::Value,
    /// Output contract schema.
    pub output_schema: serde_json::Value,
    /// Content digest the requester can independently verify.
    pub package_digest: String,
}

/// Why an agent could not satisfy a capability negotiation request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityNegotiationError {
    /// No matching capability package was advertised.
    #[error("capability is unavailable: {0}")]
    Unavailable(String),
    /// A matching package requests effects the caller did not accept.
    #[error("capability effects exceed caller policy: {0}")]
    EffectsNotAccepted(String),
    /// A matching package requires permissions the caller did not delegate.
    #[error("capability permissions were not delegated: {0}")]
    PermissionNotGranted(String),
    /// No advertised implementation can be executed by this agent.
    #[error("capability implementation is unavailable: {0}")]
    ImplementationUnavailable(String),
    /// Advertised package failed integrity or schema validation.
    #[error("capability package is invalid or untrusted: {0}")]
    InvalidPackage(String),
}

/// Negotiates a concrete offer from one agent's pre-admitted package catalog.
///
/// This is an offer only. The caller still verifies package trust, performs
/// request-time authorization, and obtains any required human approval.
pub fn negotiate_capability_offer(
    request: &CapabilityNegotiationRequest,
    catalog: &[CapabilityPackage],
) -> Result<CapabilityOffer, CapabilityNegotiationError> {
    let matching: Vec<_> = catalog
        .iter()
        .filter(|package| {
            package.capability.id == request.capability_id
                && request
                    .version
                    .as_deref()
                    .is_none_or(|version| package.capability.version == version)
        })
        .collect();
    let [package] = matching.as_slice() else {
        return Err(CapabilityNegotiationError::Unavailable(
            request.capability_id.clone(),
        ));
    };
    package
        .validate()
        .map_err(|_| CapabilityNegotiationError::InvalidPackage(package.capability.id.clone()))?;
    if package.provenance.is_some() && !package.verify_content_digest() {
        return Err(CapabilityNegotiationError::InvalidPackage(
            package.capability.id.clone(),
        ));
    }
    if package
        .contract
        .effects
        .iter()
        .any(|effect| !request.accepted_effects.contains(effect))
    {
        return Err(CapabilityNegotiationError::EffectsNotAccepted(
            package.capability.id.clone(),
        ));
    }
    if package
        .authority
        .permissions
        .iter()
        .any(|permission| !request.granted_permissions.contains(permission))
    {
        return Err(CapabilityNegotiationError::PermissionNotGranted(
            package.capability.id.clone(),
        ));
    }
    let implementation = select_implementation(
        package,
        &request.available_bindings,
        &request.preferred_kinds,
    )
    .map_err(|_| {
        CapabilityNegotiationError::ImplementationUnavailable(package.capability.id.clone())
    })?;
    let package_digest = package
        .content_digest()
        .map_err(|_| CapabilityNegotiationError::InvalidPackage(package.capability.id.clone()))?;
    Ok(CapabilityOffer {
        capability_id: package.capability.id.clone(),
        capability_version: package.capability.version.clone(),
        implementation: implementation.clone(),
        effects: package.contract.effects.clone(),
        requested_permissions: package.authority.permissions.clone(),
        input_schema: package.contract.inputs.clone(),
        output_schema: package.contract.outputs.clone(),
        package_digest,
    })
}

/// One ranked capability discovery result.
#[derive(Debug, Clone, PartialEq)]
pub struct CapabilityMatch<'a> {
    /// Candidate package.
    pub package: &'a CapabilityPackage,
    /// Deterministic lexical relevance score; this is a suggestion, not proof of compatibility.
    pub score: u32,
    /// Query terms that matched the capability ID or intent.
    pub matched_terms: Vec<String>,
}

/// Finds capabilities by natural-language intent using deterministic lexical matching.
///
/// Matching narrows a catalog. It never establishes contract equivalence,
/// grants authority, or permits execution; callers must validate schemas and
/// evaluate policy independently.
pub fn discover_capabilities<'a>(
    query: &str,
    catalog: &'a [CapabilityPackage],
    limit: usize,
) -> Vec<CapabilityMatch<'a>> {
    if query.trim().is_empty() || limit == 0 {
        return Vec::new();
    }
    let query_terms: BTreeSet<_> = tokens(query).into_iter().collect();
    if query_terms.is_empty() {
        return Vec::new();
    }
    let mut matches: Vec<_> = catalog
        .iter()
        .filter_map(|package| {
            let id_terms: BTreeSet<_> = tokens(&package.capability.id).into_iter().collect();
            let intent_terms: BTreeSet<_> =
                tokens(&package.capability.intent).into_iter().collect();
            let matched_terms: Vec<_> = query_terms
                .iter()
                .filter(|term| id_terms.contains(*term) || intent_terms.contains(*term))
                .cloned()
                .collect();
            if matched_terms.is_empty() {
                return None;
            }
            let score = matched_terms
                .iter()
                .map(|term| if id_terms.contains(term) { 3 } else { 1 })
                .sum::<u32>()
                + u32::from(
                    package
                        .capability
                        .intent
                        .to_lowercase()
                        .contains(&query.to_lowercase()),
                ) * 2;
            Some(CapabilityMatch {
                package,
                score,
                matched_terms,
            })
        })
        .collect();
    matches.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.package.capability.id.cmp(&right.package.capability.id))
            .then_with(|| {
                left.package
                    .capability
                    .version
                    .cmp(&right.package.capability.version)
            })
    });
    matches.truncate(limit);
    matches
}

fn tokens(value: &str) -> Vec<String> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Why no implementation binding could be selected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImplementationSelectionError {
    /// The package failed contract validation.
    #[error("invalid capability package: {0}")]
    InvalidPackage(String),
    /// No declared binding is available in this runtime.
    #[error("no compatible implementation is available for capability: {0}")]
    Unavailable(String),
}

/// Selects an available implementation using explicit runtime preferences.
///
/// The caller supplies binding IDs that are installed and usable, plus an
/// ordered list of supported implementation kinds. Package declaration order
/// breaks ties, so selection is reproducible. This function does not bypass
/// capability admission, approval, or request authorization.
pub fn select_implementation<'a>(
    package: &'a CapabilityPackage,
    available_bindings: &BTreeSet<String>,
    preferred_kinds: &[ImplementationKind],
) -> Result<&'a ImplementationBinding, ImplementationSelectionError> {
    package
        .validate()
        .map_err(|_| ImplementationSelectionError::InvalidPackage(package.capability.id.clone()))?;
    for preferred_kind in preferred_kinds {
        if let Some(binding) = package.implementations.iter().find(|binding| {
            &binding.kind == preferred_kind && available_bindings.contains(&binding.id)
        }) {
            return Ok(binding);
        }
    }
    Err(ImplementationSelectionError::Unavailable(
        package.capability.id.clone(),
    ))
}

/// Failure while resolving a composed capability plan.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityPlanError {
    /// No package satisfies a declared capability requirement.
    #[error("required capability is unavailable: {0}")]
    MissingCapability(String),
    /// Several versions match an unconstrained requirement.
    #[error("capability version is ambiguous; specify an exact version: {0}")]
    AmbiguousVersion(String),
    /// A dependency cycle prevents a topological execution plan.
    #[error("capability dependency cycle includes: {0}")]
    DependencyCycle(String),
    /// A selected package failed local validation.
    #[error("invalid capability package in plan: {0}")]
    InvalidPackage(String),
}

/// Resolves a root capability into dependency-first execution order.
///
/// Only exact versions are selected. An unconstrained requirement is accepted
/// when exactly one version is present; this avoids guessing whether arbitrary
/// publisher version strings follow semantic-version rules.
///
/// # Errors
///
/// Returns a missing, ambiguous, cyclic, or invalid package error. The resolver
/// does not select implementations or authorize execution; those remain policy
/// and runtime responsibilities.
pub fn resolve_capability_plan<'a>(
    root_id: &str,
    catalog: &'a [CapabilityPackage],
) -> Result<Vec<&'a CapabilityPackage>, CapabilityPlanError> {
    let mut by_id: BTreeMap<&str, Vec<&CapabilityPackage>> = BTreeMap::new();
    for package in catalog {
        by_id
            .entry(package.capability.id.as_str())
            .or_default()
            .push(package);
    }
    for packages in by_id.values_mut() {
        packages.sort_by(|left, right| left.capability.version.cmp(&right.capability.version));
    }

    let root = select_package(root_id, None, &by_id)?;
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut plan = Vec::new();
    visit(root, &by_id, &mut visiting, &mut visited, &mut plan)?;
    Ok(plan)
}

fn select_package<'a>(
    id: &str,
    version: Option<&str>,
    by_id: &BTreeMap<&str, Vec<&'a CapabilityPackage>>,
) -> Result<&'a CapabilityPackage, CapabilityPlanError> {
    let Some(packages) = by_id.get(id) else {
        return Err(CapabilityPlanError::MissingCapability(id.into()));
    };
    let matching: Vec<_> = packages
        .iter()
        .copied()
        .filter(|package| version.is_none_or(|version| package.capability.version == version))
        .collect();
    match matching.as_slice() {
        [] => Err(CapabilityPlanError::MissingCapability(match version {
            Some(version) => format!("{id}@{version}"),
            None => id.into(),
        })),
        [package] => Ok(*package),
        _ => Err(CapabilityPlanError::AmbiguousVersion(id.into())),
    }
}

fn visit<'a>(
    package: &'a CapabilityPackage,
    by_id: &BTreeMap<&str, Vec<&'a CapabilityPackage>>,
    visiting: &mut BTreeSet<String>,
    visited: &mut BTreeSet<String>,
    plan: &mut Vec<&'a CapabilityPackage>,
) -> Result<(), CapabilityPlanError> {
    package
        .validate()
        .map_err(|_| CapabilityPlanError::InvalidPackage(package.capability.id.clone()))?;
    let key = format!("{}@{}", package.capability.id, package.capability.version);
    if visited.contains(&key) {
        return Ok(());
    }
    if !visiting.insert(key.clone()) {
        return Err(CapabilityPlanError::DependencyCycle(key));
    }
    for requirement in &package.contract.requires {
        let dependency = select_package(&requirement.id, requirement.version.as_deref(), by_id)?;
        visit(dependency, by_id, visiting, visited, plan)?;
    }
    visiting.remove(&key);
    visited.insert(key);
    plan.push(package);
    Ok(())
}
