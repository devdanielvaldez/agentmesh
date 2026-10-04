//! `AgentMesh` Teach foundations: the Workflow IR and its validator.
//!
//! The IR is the portable contract between every Teach component: recorders
//! produce it (via semantic traces), the validator accepts or rejects it,
//! runtimes execute it, and compilers turn it into MCP tools, CLIs, or REST
//! endpoints. This crate owns the schema and the static checks; storage,
//! inference, and execution live in later commits.

mod ir;

pub use ir::{
    ApiObservation, InputDef, InputType, KNOWN_OPS, OutputDef, RecoveryPolicy,
    SUPPORTED_IR_VERSION, Step, Target, Workflow, WorkflowAssertion, WorkflowPolicy,
    extract_template_refs, parse_workflow, relax_workflow, validate_workflow,
};
use thiserror::Error;

/// Teach failure: unparsable YAML, invalid IR, unknown workflow, or storage error.
#[derive(Debug, Error)]
pub enum TeachError {
    /// YAML could not be parsed into a workflow.
    #[error("invalid workflow YAML: {0}")]
    Parse(#[from] serde_yaml::Error),
    /// The workflow parsed but failed static validation.
    #[error("invalid workflow: {0}")]
    Validation(String),
    /// No stored workflow has the requested id.
    #[error("unknown workflow: {0}")]
    NotFound(String),
    /// The workflow store could not be read or written.
    #[error("workflow storage error: {0}")]
    Storage(#[from] std::io::Error),
}
