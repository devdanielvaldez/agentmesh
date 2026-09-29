//! Supervised child-process transport for local MCP servers.

use crate::StdioTransport;
use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_protocol::{JsonRpcMessage, ProtocolLimits};
use std::{collections::BTreeSet, path::PathBuf};
use tokio::process::{Child, Command};

/// Validated process launch specification.
#[derive(Debug, Clone)]
pub struct ProcessSpec {
    /// Executable path.
    pub program: PathBuf,
    /// Bounded arguments.
    pub arguments: Vec<String>,
    /// Explicit environment allowlist.
    pub environment: Vec<(String, String)>,
}
impl ProcessSpec {
    /// Ensures launch input is bounded and the executable is allowlisted.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] for denied executables or excessive arguments.
    pub fn validate(&self, allowed_programs: &BTreeSet<PathBuf>) -> Result<(), AgentMeshError> {
        if !allowed_programs.contains(&self.program)
            || self.arguments.len() > 128
            || self.environment.len() > 128
            || self.arguments.iter().any(|value| value.len() > 4096)
        {
            return Err(AgentMeshError::new(
                ErrorCode::PermissionDenied,
                "The local MCP process specification is not allowed.",
            ));
        }
        Ok(())
    }
}

/// Child process with bounded MCP stdin/stdout framing.
pub struct ChildProcessTransport {
    child: Child,
    transport: StdioTransport<tokio::process::ChildStdout, tokio::process::ChildStdin>,
}
impl ChildProcessTransport {
    /// Spawns a process with inherited environment cleared.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] when validation or process startup fails.
    pub fn spawn(
        spec: &ProcessSpec,
        allowed_programs: &BTreeSet<PathBuf>,
        limits: ProtocolLimits,
    ) -> Result<Self, AgentMeshError> {
        spec.validate(allowed_programs)?;
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.arguments)
            .env_clear()
            .envs(spec.environment.iter().cloned())
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let mut child = command.spawn().map_err(|error| {
            AgentMeshError::with_source(
                ErrorCode::UpstreamConnectionFailed,
                "The local MCP process could not start.",
                error,
            )
        })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            AgentMeshError::new(
                ErrorCode::UpstreamConnectionFailed,
                "The local MCP process has no stdin.",
            )
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AgentMeshError::new(
                ErrorCode::UpstreamConnectionFailed,
                "The local MCP process has no stdout.",
            )
        })?;
        Ok(Self {
            child,
            transport: StdioTransport::new(stdout, stdin, limits),
        })
    }
    /// Sends one message.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] for framing or process I/O failures.
    pub async fn send(&mut self, message: &JsonRpcMessage) -> Result<(), AgentMeshError> {
        self.transport.send_message(message).await
    }
    /// Receives one message.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] for framing or process I/O failures.
    pub async fn receive(&mut self) -> Result<Option<JsonRpcMessage>, AgentMeshError> {
        self.transport.receive_message().await
    }
    /// Gracefully closes stdin then terminates a process that remains alive.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] when shutdown or termination fails.
    pub async fn shutdown(&mut self) -> Result<(), AgentMeshError> {
        self.transport.shutdown().await?;
        if self
            .child
            .try_wait()
            .map_err(AgentMeshError::internal)?
            .is_none()
        {
            self.child.kill().await.map_err(AgentMeshError::internal)?;
        }
        Ok(())
    }
}
