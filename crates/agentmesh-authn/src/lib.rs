//! Pluggable authentication with normalized principals and redacted credentials.

use std::{collections::BTreeSet, fmt, sync::Arc};

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_registry::RegistryScope;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Maximum verifier chain length.
pub const MAX_AUTHENTICATION_VERIFIERS: usize = 32;
/// Maximum locally configured API keys.
pub const MAX_API_KEYS: usize = 100_000;
/// Maximum bytes accepted for one presented credential.
pub const MAX_CREDENTIAL_BYTES: usize = 16_384;

/// Credential material that never exposes its value through formatting.
pub struct SecretCredential(String);

impl SecretCredential {
    /// Wraps bounded credential material.
    ///
    /// # Errors
    ///
    /// Rejects empty or oversized values.
    pub fn new(value: impl Into<String>) -> Result<Self, AgentMeshError> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_CREDENTIAL_BYTES {
            return Err(AgentMeshError::new(
                ErrorCode::InvalidCredentials,
                "The credential is invalid.",
            ));
        }
        Ok(Self(value))
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretCredential([REDACTED])")
    }
}

/// One explicitly selected authentication mechanism.
#[derive(Debug)]
pub enum PresentedCredential {
    /// Opaque `AgentMesh` API key.
    ApiKey(SecretCredential),
    /// OAuth/JWT bearer token delegated to a configured verifier.
    Bearer(SecretCredential),
    /// Validated mTLS certificate subject from the transport.
    MutualTls {
        /// Certificate subject already validated by the TLS transport.
        subject: String,
    },
    /// Validated workload identity, such as a SPIFFE ID.
    Workload {
        /// Workload identity already validated by its transport/verifier.
        identity: String,
    },
}

/// Input to the verifier chain.
pub struct AuthenticationRequest<'a> {
    /// Tenant boundary selected before resource lookup.
    pub scope: &'a RegistryScope,
    /// Presented mechanism.
    pub credential: &'a PresentedCredential,
    /// Monotonic or Unix epoch milliseconds chosen consistently by the caller.
    pub now_millis: u64,
}

/// Relative assurance of a successful authentication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthenticationStrength {
    /// Long-lived shared secret.
    Basic,
    /// Signed or short-lived token.
    Strong,
    /// Hardware/transport-bound workload identity.
    HardwareBound,
}

/// Sanitized evidence metadata safe for audit records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthenticationEvidence {
    /// Mechanism that succeeded.
    pub mechanism: String,
    /// Non-secret key, issuer, certificate, or workload reference.
    pub reference: String,
}

/// Normalized identity consumed by authorization and policy modules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthenticatedPrincipal {
    /// Stable principal identifier.
    pub id: String,
    /// Tenant boundary authenticated by the verifier.
    pub scope: RegistryScope,
    /// Authentication assurance.
    pub strength: AuthenticationStrength,
    /// Roles asserted by the identity source; authorization still decides access.
    pub asserted_roles: BTreeSet<String>,
    /// Safe evidence for explanations and audit.
    pub evidence: AuthenticationEvidence,
}

/// Outcome lets chains distinguish an unrelated mechanism from rejection.
pub enum Verification {
    /// This verifier does not handle the presented mechanism.
    NotApplicable,
    /// The mechanism was handled and rejected.
    Rejected,
    /// A normalized principal was authenticated.
    Authenticated(AuthenticatedPrincipal),
}

/// Synchronous verifier contract; implementations may internally use refreshed snapshots.
pub trait CredentialVerifier: Send + Sync {
    /// Verifies one request without performing authorization.
    ///
    /// # Errors
    ///
    /// Returns availability/configuration errors distinct from invalid credentials.
    fn verify(&self, request: &AuthenticationRequest<'_>) -> Result<Verification, AgentMeshError>;
}

/// Ordered bounded chain of independent verifier implementations.
pub struct Authenticator {
    verifiers: Vec<Arc<dyn CredentialVerifier>>,
}

impl Authenticator {
    /// Creates a non-empty bounded chain.
    ///
    /// # Errors
    ///
    /// Rejects empty or excessive verifier lists.
    pub fn new(verifiers: Vec<Arc<dyn CredentialVerifier>>) -> Result<Self, AgentMeshError> {
        if verifiers.is_empty() || verifiers.len() > MAX_AUTHENTICATION_VERIFIERS {
            return Err(configuration(
                "The authentication verifier chain is empty or unbounded.",
            ));
        }
        Ok(Self { verifiers })
    }

    /// Authenticates exactly one principal or returns a disclosure-safe error.
    ///
    /// # Errors
    ///
    /// Returns invalid credentials after rejection, unauthenticated when no verifier applies,
    /// or a verifier availability/configuration error.
    pub fn authenticate(
        &self,
        request: &AuthenticationRequest<'_>,
    ) -> Result<AuthenticatedPrincipal, AgentMeshError> {
        let mut rejected = false;
        for verifier in &self.verifiers {
            match verifier.verify(request)? {
                Verification::NotApplicable => {}
                Verification::Rejected => rejected = true,
                Verification::Authenticated(principal) => {
                    validate_principal(&principal)?;
                    if &principal.scope != request.scope {
                        return Err(AgentMeshError::new(
                            ErrorCode::PermissionDenied,
                            "The credential is not valid for this tenant.",
                        ));
                    }
                    return Ok(principal);
                }
            }
        }
        Err(AgentMeshError::new(
            if rejected {
                ErrorCode::InvalidCredentials
            } else {
                ErrorCode::Unauthenticated
            },
            "Authentication failed.",
        ))
    }
}

/// Hashed API-key record. Plaintext is never retained.
#[derive(Clone)]
pub struct ApiKeyRecord {
    key_id: String,
    digest: [u8; 32],
    scope: RegistryScope,
    principal_id: String,
    roles: BTreeSet<String>,
    expires_at_millis: Option<u64>,
    disabled: bool,
}

impl fmt::Debug for ApiKeyRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApiKeyRecord")
            .field("key_id", &self.key_id)
            .field("digest", &"[REDACTED]")
            .field("scope", &self.scope)
            .field("principal_id", &self.principal_id)
            .field("roles", &self.roles)
            .field("expires_at_millis", &self.expires_at_millis)
            .field("disabled", &self.disabled)
            .finish()
    }
}

impl ApiKeyRecord {
    /// Hashes a plaintext key into a scoped record.
    ///
    /// # Errors
    ///
    /// Rejects invalid identifiers, roles, or credential sizes.
    pub fn from_plaintext(
        key_id: impl Into<String>,
        plaintext: &SecretCredential,
        scope: RegistryScope,
        principal_id: impl Into<String>,
        roles: BTreeSet<String>,
        expires_at_millis: Option<u64>,
    ) -> Result<Self, AgentMeshError> {
        let key_id = key_id.into();
        let principal_id = principal_id.into();
        validate_text(&key_id, 256)?;
        validate_text(&principal_id, 256)?;
        for role in &roles {
            validate_text(role, 256)?;
        }
        let digest: [u8; 32] = Sha256::digest(plaintext.expose().as_bytes()).into();
        Ok(Self {
            key_id,
            digest,
            scope,
            principal_id,
            roles,
            expires_at_millis,
            disabled: false,
        })
    }

    /// Marks this key disabled before building a new immutable verifier snapshot.
    pub fn disable(&mut self) {
        self.disabled = true;
    }
}

/// Immutable local API-key verifier.
pub struct ApiKeyVerifier {
    records: Vec<ApiKeyRecord>,
}

impl ApiKeyVerifier {
    /// Builds a validated key snapshot.
    ///
    /// # Errors
    ///
    /// Rejects excessive or duplicate key IDs.
    pub fn new(records: Vec<ApiKeyRecord>) -> Result<Self, AgentMeshError> {
        if records.len() > MAX_API_KEYS {
            return Err(configuration(
                "The API-key snapshot exceeds its hard limit.",
            ));
        }
        let mut ids = BTreeSet::new();
        if records
            .iter()
            .any(|record| !ids.insert(record.key_id.as_str()))
        {
            return Err(configuration("API-key identifiers must be unique."));
        }
        Ok(Self { records })
    }
}

impl CredentialVerifier for ApiKeyVerifier {
    fn verify(&self, request: &AuthenticationRequest<'_>) -> Result<Verification, AgentMeshError> {
        let PresentedCredential::ApiKey(key) = request.credential else {
            return Ok(Verification::NotApplicable);
        };
        let candidate: [u8; 32] = Sha256::digest(key.expose().as_bytes()).into();
        let Some(record) = self
            .records
            .iter()
            .find(|record| constant_time_eq(&record.digest, &candidate))
        else {
            return Ok(Verification::Rejected);
        };
        if record.disabled
            || record
                .expires_at_millis
                .is_some_and(|expiry| expiry <= request.now_millis)
        {
            return Ok(Verification::Rejected);
        }
        Ok(Verification::Authenticated(AuthenticatedPrincipal {
            id: record.principal_id.clone(),
            scope: record.scope.clone(),
            strength: AuthenticationStrength::Basic,
            asserted_roles: record.roles.clone(),
            evidence: AuthenticationEvidence {
                mechanism: "api_key".into(),
                reference: record.key_id.clone(),
            },
        }))
    }
}

fn constant_time_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn validate_principal(principal: &AuthenticatedPrincipal) -> Result<(), AgentMeshError> {
    validate_text(&principal.id, 256)?;
    validate_text(&principal.evidence.mechanism, 64)?;
    validate_text(&principal.evidence.reference, 1_024)?;
    for role in &principal.asserted_roles {
        validate_text(role, 256)?;
    }
    Ok(())
}

fn validate_text(value: &str, max: usize) -> Result<(), AgentMeshError> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(configuration(
            "An authentication identity value is invalid or unbounded.",
        ));
    }
    Ok(())
}

fn configuration(message: &'static str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::ConfigurationInvalid, message)
}
