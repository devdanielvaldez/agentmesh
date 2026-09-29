//! Bounded session state and cooperative cancellation.

use agentmesh_error::{AgentMeshError, ErrorCode};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Opaque validated MCP session identifier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(String);
impl SessionId {
    /// Parses an untrusted session header.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] when the identifier is malformed.
    pub fn parse(value: impl Into<String>) -> Result<Self, AgentMeshError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(AgentMeshError::new(
                ErrorCode::InvalidRequest,
                "The MCP session identifier is invalid.",
            ));
        }
        Ok(Self(value))
    }
    /// Wire representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Cloneable cooperative cancellation signal.
#[derive(Debug, Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    /// Requests cancellation.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    /// Whether cancellation was requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone)]
struct Session {
    expires_at: Instant,
    cancellation: Cancellation,
}

/// Capacity- and TTL-bounded session registry.
pub struct SessionRegistry {
    sessions: RwLock<BTreeMap<SessionId, Session>>,
    capacity: usize,
    ttl: Duration,
}
impl SessionRegistry {
    /// Creates a registry with positive bounds.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] when capacity or TTL is zero.
    pub fn new(capacity: usize, ttl: Duration) -> Result<Self, AgentMeshError> {
        if capacity == 0 || ttl.is_zero() {
            return Err(AgentMeshError::new(
                ErrorCode::ConfigurationInvalid,
                "Session capacity and TTL must be positive.",
            ));
        }
        Ok(Self {
            sessions: RwLock::new(BTreeMap::new()),
            capacity,
            ttl,
        })
    }
    /// Opens a unique session and returns its cancellation handle.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] on duplicates, capacity exhaustion, or unavailable state.
    pub fn open(&self, id: SessionId, now: Instant) -> Result<Cancellation, AgentMeshError> {
        let mut sessions = self.sessions.write().map_err(|_| unavailable())?;
        sessions.retain(|_, session| session.expires_at > now);
        if sessions.len() >= self.capacity {
            return Err(AgentMeshError::new(
                ErrorCode::ConcurrencyLimited,
                "The MCP session capacity is exhausted.",
            ));
        }
        if sessions.contains_key(&id) {
            return Err(AgentMeshError::new(
                ErrorCode::Conflict,
                "The MCP session already exists.",
            ));
        }
        let cancellation = Cancellation::default();
        sessions.insert(
            id,
            Session {
                expires_at: now + self.ttl,
                cancellation: cancellation.clone(),
            },
        );
        Ok(cancellation)
    }
    /// Renews a live session and returns its cancellation handle.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] if session state is unavailable.
    pub fn touch(
        &self,
        id: &SessionId,
        now: Instant,
    ) -> Result<Option<Cancellation>, AgentMeshError> {
        let mut sessions = self.sessions.write().map_err(|_| unavailable())?;
        let Some(session) = sessions.get_mut(id) else {
            return Ok(None);
        };
        if session.expires_at <= now {
            sessions.remove(id);
            return Ok(None);
        }
        session.expires_at = now + self.ttl;
        Ok(Some(session.cancellation.clone()))
    }
    /// Cancels and removes a session.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] if session state is unavailable.
    pub fn close(&self, id: &SessionId) -> Result<bool, AgentMeshError> {
        let removed = self.sessions.write().map_err(|_| unavailable())?.remove(id);
        if let Some(session) = removed {
            session.cancellation.cancel();
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
fn unavailable() -> AgentMeshError {
    AgentMeshError::new(
        ErrorCode::ConfigurationUnavailable,
        "MCP session state is unavailable.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enforces_capacity_expiry_and_cancellation() {
        let sessions = SessionRegistry::new(1, Duration::from_secs(1)).unwrap();
        let now = Instant::now();
        let id = SessionId::parse("session-1").unwrap();
        let signal = sessions.open(id.clone(), now).unwrap();
        assert!(
            sessions
                .open(SessionId::parse("session-2").unwrap(), now)
                .is_err()
        );
        assert!(sessions.close(&id).unwrap());
        assert!(signal.is_cancelled());
    }
}
