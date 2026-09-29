//! Deterministic component assembly, readiness, and reverse-order shutdown.

use agentmesh_error::{AgentMeshError, ErrorCode};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Supported deployment compositions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeProfile {
    /// Local developer profile.
    Dev,
    /// Single-process production profile.
    Standalone,
    /// Data-plane-only profile.
    Gateway,
    /// Control-plane-only profile.
    ControlPlane,
    /// Combined control and data plane.
    AllInOne,
}

/// Current component readiness state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentStatus {
    /// Component is not running.
    Stopped,
    /// Component is initializing.
    Starting,
    /// Component can serve traffic.
    Ready,
    /// Component can serve reduced functionality.
    Degraded,
    /// Component cannot operate.
    Failed,
}

/// Snapshot exposed by readiness and diagnostics endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeStatus {
    /// Overall readiness.
    pub ready: bool,
    /// Per-component state.
    pub components: BTreeMap<String, ComponentStatus>,
}

/// Lifecycle contract implemented by runtime components.
pub trait Component: Send {
    /// Stable component name.
    fn name(&self) -> &str;
    /// Names that must start first.
    fn dependencies(&self) -> &[String];
    /// Starts and validates the component.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] when initialization fails.
    fn start(&mut self) -> Result<(), AgentMeshError>;
    /// Reports live readiness without side effects.
    fn status(&self) -> ComponentStatus;
    /// Stops accepting work and releases resources.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] when shutdown cannot complete cleanly.
    fn stop(&mut self) -> Result<(), AgentMeshError>;
}

/// Owns component lifecycles for one process profile.
pub struct Runtime {
    profile: RuntimeProfile,
    components: BTreeMap<String, Box<dyn Component>>,
    start_order: Vec<String>,
}

impl Runtime {
    /// Creates an empty runtime profile.
    pub fn new(profile: RuntimeProfile) -> Self {
        Self {
            profile,
            components: BTreeMap::new(),
            start_order: Vec::new(),
        }
    }
    /// Active deployment profile.
    pub const fn profile(&self) -> RuntimeProfile {
        self.profile
    }

    /// Registers one uniquely named component before startup.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] for duplicate names or registration after startup.
    pub fn register(&mut self, component: impl Component + 'static) -> Result<(), AgentMeshError> {
        if !self.start_order.is_empty() {
            return Err(conflict("Components cannot be registered after startup."));
        }
        let name = component.name().to_owned();
        if name.is_empty() || name.len() > 128 || self.components.contains_key(&name) {
            return Err(conflict("Component names must be unique and bounded."));
        }
        self.components.insert(name, Box::new(component));
        Ok(())
    }

    /// Starts all components in topological order, rolling back on failure.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] for invalid dependencies or startup failure.
    pub fn start(&mut self) -> Result<(), AgentMeshError> {
        let order = resolve_order(&self.components)?;
        for name in order {
            let result = self
                .components
                .get_mut(&name)
                .ok_or_else(|| {
                    AgentMeshError::new(
                        ErrorCode::ConfigurationInvalid,
                        "A resolved runtime component is missing.",
                    )
                })?
                .start();
            if let Err(error) = result {
                self.rollback();
                return Err(error);
            }
            self.start_order.push(name);
        }
        Ok(())
    }

    /// Reports ready only when every component is ready or explicitly degraded.
    pub fn status(&self) -> RuntimeStatus {
        let components: BTreeMap<_, _> = self
            .components
            .iter()
            .map(|(name, component)| (name.clone(), component.status()))
            .collect();
        let ready = !components.is_empty()
            && components
                .values()
                .all(|status| matches!(status, ComponentStatus::Ready | ComponentStatus::Degraded));
        RuntimeStatus { ready, components }
    }

    /// Stops started components in exact reverse dependency order.
    ///
    /// # Errors
    ///
    /// Returns the first component shutdown failure after attempting every stop.
    pub fn shutdown(&mut self) -> Result<(), AgentMeshError> {
        let mut first_error = None;
        while let Some(name) = self.start_order.pop() {
            if let Some(component) = self.components.get_mut(&name) {
                if let Err(error) = component.stop() {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn rollback(&mut self) {
        while let Some(name) = self.start_order.pop() {
            if let Some(component) = self.components.get_mut(&name) {
                let _ = component.stop();
            }
        }
    }
}

fn resolve_order(
    components: &BTreeMap<String, Box<dyn Component>>,
) -> Result<Vec<String>, AgentMeshError> {
    let mut resolved = Vec::with_capacity(components.len());
    let mut remaining: BTreeSet<_> = components.keys().cloned().collect();
    while !remaining.is_empty() {
        let ready: Vec<_> = remaining
            .iter()
            .filter(|name| {
                components[*name]
                    .dependencies()
                    .iter()
                    .all(|dependency| resolved.contains(dependency))
            })
            .cloned()
            .collect();
        if ready.is_empty() {
            return Err(AgentMeshError::new(
                ErrorCode::ConfigurationInvalid,
                "Runtime dependencies are missing or cyclic.",
            ));
        }
        for name in ready {
            remaining.remove(&name);
            resolved.push(name);
        }
    }
    Ok(resolved)
}
fn conflict(message: &str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::Conflict, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    struct TestComponent {
        name: String,
        dependencies: Vec<String>,
        state: ComponentStatus,
        events: Arc<Mutex<Vec<String>>>,
    }
    impl Component for TestComponent {
        fn name(&self) -> &str {
            &self.name
        }
        fn dependencies(&self) -> &[String] {
            &self.dependencies
        }
        fn start(&mut self) -> Result<(), AgentMeshError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("start:{}", self.name));
            self.state = ComponentStatus::Ready;
            Ok(())
        }
        fn status(&self) -> ComponentStatus {
            self.state
        }
        fn stop(&mut self) -> Result<(), AgentMeshError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("stop:{}", self.name));
            self.state = ComponentStatus::Stopped;
            Ok(())
        }
    }
    #[test]
    fn starts_dependencies_and_stops_in_reverse() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let make = |name: &str, dependencies: &[&str]| TestComponent {
            name: name.into(),
            dependencies: dependencies.iter().map(ToString::to_string).collect(),
            state: ComponentStatus::Stopped,
            events: Arc::clone(&events),
        };
        let mut runtime = Runtime::new(RuntimeProfile::AllInOne);
        runtime.register(make("gateway", &["storage"])).unwrap();
        runtime.register(make("storage", &[])).unwrap();
        runtime.start().unwrap();
        assert!(runtime.status().ready);
        runtime.shutdown().unwrap();
        assert_eq!(
            *events.lock().unwrap(),
            [
                "start:storage",
                "start:gateway",
                "stop:gateway",
                "stop:storage"
            ]
        );
    }
}
