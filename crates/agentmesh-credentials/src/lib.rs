//! Opaque secret references, pluggable providers, and short-lived credential brokering.

use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    sync::{Arc, Mutex, MutexGuard},
};

use agentmesh_core::ServerId;
use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_registry::RegistryScope;
use serde::{Deserialize, Serialize};

/// Maximum secret payload accepted from a provider.
pub const MAX_SECRET_BYTES: usize = 64 * 1_024;
/// Maximum cached secrets in one broker.
pub const MAX_CACHED_SECRETS: usize = 10_000;
/// Maximum selection rules in one broker snapshot.
pub const MAX_CREDENTIAL_RULES: usize = 10_000;
/// Maximum undrained broker events.
pub const MAX_CREDENTIAL_EVENTS: usize = 10_000;

/// Persistent opaque reference; plaintext is never part of configuration state.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretReference {
    /// Registered provider name.
    pub provider: String,
    /// Provider-specific opaque key/path.
    pub key: String,
    /// Optional immutable provider version.
    pub version: Option<String>,
}

impl SecretReference {
    /// Creates a validated secret reference.
    ///
    /// # Errors
    ///
    /// Rejects empty, control-character, or oversized reference components.
    pub fn new(
        provider: impl Into<String>,
        key: impl Into<String>,
        version: Option<String>,
    ) -> Result<Self, AgentMeshError> {
        let reference = Self {
            provider: provider.into(),
            key: key.into(),
            version,
        };
        validate_text(&reference.provider, 128)?;
        validate_text(&reference.key, 1_024)?;
        if let Some(version) = &reference.version {
            validate_text(version, 256)?;
        }
        Ok(reference)
    }
}

/// Secret bytes that redact formatting and are overwritten on drop.
pub struct SecretMaterial {
    bytes: Box<[u8]>,
}

impl SecretMaterial {
    /// Wraps a bounded non-empty provider result.
    ///
    /// # Errors
    ///
    /// Rejects empty or excessive payloads.
    pub fn new(bytes: impl Into<Vec<u8>>) -> Result<Self, AgentMeshError> {
        let bytes = bytes.into();
        if bytes.is_empty() || bytes.len() > MAX_SECRET_BYTES {
            return Err(AgentMeshError::new(
                ErrorCode::InvalidCredentials,
                "Secret material is empty or exceeds its size limit.",
            ));
        }
        Ok(Self {
            bytes: bytes.into_boxed_slice(),
        })
    }

    /// Exposes bytes only to a credential-aware adapter.
    pub fn expose(&self) -> &[u8] {
        &self.bytes
    }
}

impl fmt::Debug for SecretMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretMaterial([REDACTED])")
    }
}

impl Drop for SecretMaterial {
    fn drop(&mut self) {
        self.bytes.fill(0);
    }
}

/// Provider output including a cache expiry and safe revision reference.
pub struct ProviderSecret {
    /// Sensitive bytes.
    pub material: SecretMaterial,
    /// Monotonic time after which the value must be re-resolved.
    pub expires_at_millis: u64,
    /// Safe provider revision for rotation events.
    pub revision: String,
}

/// Pluggable environment, Vault, Kubernetes, or cloud provider contract.
pub trait SecretProvider: Send + Sync {
    /// Resolves one reference.
    ///
    /// # Errors
    ///
    /// Returns invalid-credential or availability errors without leaking provider details.
    fn resolve(
        &self,
        reference: &SecretReference,
        now_millis: u64,
    ) -> Result<ProviderSecret, AgentMeshError>;
}

/// Credential-selection rule for one tenant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialRule {
    /// Stable rule ID.
    pub id: String,
    /// Higher values take precedence.
    pub priority: u32,
    /// Optional logical service constraint.
    pub service: Option<ServerId>,
    /// Optional exact capability constraint.
    pub capability: Option<String>,
    /// Secret selected by this rule.
    pub secret: SecretReference,
}

/// Safe broker event for telemetry, audit, rotation, and revocation handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialEvent {
    /// Rule that selected the credential.
    pub rule_id: String,
    /// Provider name.
    pub provider: String,
    /// Safe provider revision.
    pub revision: String,
    /// Whether provider I/O was avoided.
    pub cache_hit: bool,
    /// Event timestamp.
    pub at_millis: u64,
}

struct CachedSecret {
    material: Arc<SecretMaterial>,
    expires_at_millis: u64,
    revision: String,
}

/// Short-lived secret lease; cloning only shares the same redacted material.
pub struct CredentialLease {
    material: Arc<SecretMaterial>,
    /// Selection rule ID.
    pub rule_id: String,
    /// Safe provider revision.
    pub revision: String,
}

impl CredentialLease {
    /// Exposes the credential only at the upstream adapter boundary.
    pub fn expose(&self) -> &[u8] {
        self.material.expose()
    }
}

impl fmt::Debug for CredentialLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialLease")
            .field("material", &"[REDACTED]")
            .field("rule_id", &self.rule_id)
            .field("revision", &self.revision)
            .finish()
    }
}

/// Immutable selection snapshot plus bounded rotating cache.
pub struct CredentialBroker {
    scope: RegistryScope,
    rules: Vec<CredentialRule>,
    providers: BTreeMap<String, Arc<dyn SecretProvider>>,
    cache: Mutex<BTreeMap<SecretReference, CachedSecret>>,
    events: Mutex<VecDeque<CredentialEvent>>,
}

impl CredentialBroker {
    /// Builds a validated tenant broker.
    ///
    /// # Errors
    ///
    /// Rejects duplicate rules, unknown providers, and unbounded snapshots.
    pub fn new(
        scope: RegistryScope,
        mut rules: Vec<CredentialRule>,
        providers: BTreeMap<String, Arc<dyn SecretProvider>>,
    ) -> Result<Self, AgentMeshError> {
        if rules.is_empty() || rules.len() > MAX_CREDENTIAL_RULES || providers.is_empty() {
            return Err(configuration(
                "Credential rules/providers are empty or unbounded.",
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        for rule in &rules {
            validate_text(&rule.id, 256)?;
            if !ids.insert(rule.id.as_str()) || !providers.contains_key(&rule.secret.provider) {
                return Err(configuration(
                    "Credential rules are duplicate or reference an unknown provider.",
                ));
            }
            if let Some(capability) = &rule.capability {
                validate_text(capability, 1_024)?;
            }
        }
        rules.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| right.capability.is_some().cmp(&left.capability.is_some()))
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(Self {
            scope,
            rules,
            providers,
            cache: Mutex::new(BTreeMap::new()),
            events: Mutex::new(VecDeque::new()),
        })
    }

    /// Selects and resolves the credential for an exact tenant/service/capability context.
    ///
    /// # Errors
    ///
    /// Returns tenant, selection, provider, capacity, or expiry errors.
    pub fn resolve(
        &self,
        scope: &RegistryScope,
        service: ServerId,
        capability: Option<&str>,
        now_millis: u64,
    ) -> Result<CredentialLease, AgentMeshError> {
        if scope != &self.scope {
            return Err(AgentMeshError::new(
                ErrorCode::PermissionDenied,
                "Credential access crossed a tenant boundary.",
            ));
        }
        let rule = self
            .rules
            .iter()
            .find(|rule| {
                rule.service.is_none_or(|value| value == service)
                    && rule
                        .capability
                        .as_deref()
                        .is_none_or(|value| Some(value) == capability)
            })
            .ok_or_else(|| {
                AgentMeshError::new(
                    ErrorCode::InvalidCredentials,
                    "No credential policy matches the upstream request.",
                )
            })?;
        if let Some(cached) = lock(&self.cache)
            .get(&rule.secret)
            .filter(|cached| cached.expires_at_millis > now_millis)
        {
            let lease = CredentialLease {
                material: Arc::clone(&cached.material),
                rule_id: rule.id.clone(),
                revision: cached.revision.clone(),
            };
            self.publish(rule, &lease.revision, true, now_millis);
            return Ok(lease);
        }
        let provider = self
            .providers
            .get(&rule.secret.provider)
            .ok_or_else(|| configuration("The selected secret provider is unavailable."))?;
        let resolved = provider.resolve(&rule.secret, now_millis)?;
        validate_text(&resolved.revision, 256)?;
        if resolved.expires_at_millis <= now_millis {
            return Err(AgentMeshError::new(
                ErrorCode::CredentialsExpired,
                "The resolved credential has expired.",
            ));
        }
        let material = Arc::new(resolved.material);
        let mut cache = lock(&self.cache);
        if !cache.contains_key(&rule.secret) && cache.len() >= MAX_CACHED_SECRETS {
            return Err(AgentMeshError::new(
                ErrorCode::StorageUnavailable,
                "The credential cache is at capacity.",
            ));
        }
        cache.insert(
            rule.secret.clone(),
            CachedSecret {
                material: Arc::clone(&material),
                expires_at_millis: resolved.expires_at_millis,
                revision: resolved.revision.clone(),
            },
        );
        drop(cache);
        let lease = CredentialLease {
            material,
            rule_id: rule.id.clone(),
            revision: resolved.revision,
        };
        self.publish(rule, &lease.revision, false, now_millis);
        Ok(lease)
    }

    /// Revokes one reference from the cache. Existing in-flight leases expire by ownership.
    pub fn revoke(&self, reference: &SecretReference) -> bool {
        lock(&self.cache).remove(reference).is_some()
    }

    /// Removes expired cached secrets in bounded batches.
    pub fn purge_expired(&self, now_millis: u64, limit: usize) -> usize {
        let mut cache = lock(&self.cache);
        let keys = cache
            .iter()
            .filter(|(_, value)| value.expires_at_millis <= now_millis)
            .take(limit)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        let removed = keys.len();
        for key in keys {
            cache.remove(&key);
        }
        removed
    }

    /// Drains safe broker events.
    pub fn drain_events(&self) -> Vec<CredentialEvent> {
        lock(&self.events).drain(..).collect()
    }

    fn publish(&self, rule: &CredentialRule, revision: &str, cache_hit: bool, at_millis: u64) {
        let mut events = lock(&self.events);
        if events.len() == MAX_CREDENTIAL_EVENTS {
            events.pop_front();
        }
        events.push_back(CredentialEvent {
            rule_id: rule.id.clone(),
            provider: rule.secret.provider.clone(),
            revision: revision.into(),
            cache_hit,
            at_millis,
        });
    }
}

fn validate_text(value: &str, max: usize) -> Result<(), AgentMeshError> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(configuration(
            "A credential reference value is invalid or unbounded.",
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

/// Deterministic in-memory provider for local mode and tests.
#[derive(Default)]
pub struct InMemorySecretProvider {
    secrets: Mutex<BTreeMap<SecretReference, StoredSecret>>,
}

struct StoredSecret {
    material: Vec<u8>,
    expires_at_millis: u64,
    revision: String,
}

impl InMemorySecretProvider {
    /// Inserts or rotates a local secret.
    ///
    /// # Errors
    ///
    /// Enforces the same payload and revision limits as remote providers.
    pub fn insert(
        &self,
        reference: SecretReference,
        material: &SecretMaterial,
        expires_at_millis: u64,
        revision: impl Into<String>,
    ) -> Result<(), AgentMeshError> {
        let revision = revision.into();
        validate_text(&revision, 256)?;
        lock(&self.secrets).insert(
            reference,
            StoredSecret {
                material: material.expose().to_vec(),
                expires_at_millis,
                revision,
            },
        );
        Ok(())
    }
}

impl SecretProvider for InMemorySecretProvider {
    fn resolve(
        &self,
        reference: &SecretReference,
        _now_millis: u64,
    ) -> Result<ProviderSecret, AgentMeshError> {
        let secrets = lock(&self.secrets);
        let secret = secrets.get(reference).ok_or_else(|| {
            AgentMeshError::new(
                ErrorCode::InvalidCredentials,
                "The referenced credential is unavailable.",
            )
        })?;
        Ok(ProviderSecret {
            material: SecretMaterial::new(secret.material.clone())?,
            expires_at_millis: secret.expires_at_millis,
            revision: secret.revision.clone(),
        })
    }
}
