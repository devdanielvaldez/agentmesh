//! `AgentMesh` Teach foundations: the Workflow IR, its validator, semantic
//! traces, and local workflow storage.
//!
//! The IR is the portable contract between every Teach component: recorders
//! produce it (via semantic traces), the validator accepts or rejects it,
//! runtimes execute it, and compilers turn it into MCP tools, CLIs, or REST
//! endpoints. Inference and execution live in later commits.

mod ir;
mod store;
mod trace;

pub use ir::{
    ApiObservation, InputDef, InputType, KNOWN_OPS, OutputDef, RecoveryPolicy,
    SUPPORTED_IR_VERSION, Step, Target, Workflow, WorkflowAssertion, WorkflowPolicy,
    extract_template_refs, parse_workflow, relax_workflow, validate_workflow,
};
pub use store::{
    WorkflowListError, WorkflowSummary, delete_workflow, delete_workflow_in, home_dir,
    import_workflow_in, list_workflows, list_workflows_lenient, load_workflow, rename_workflow,
    rename_workflow_in, save_workflow, workflows_dir,
};
use thiserror::Error;
pub use trace::{RecordedTarget, SemanticEvent, TraceKind, TraceValue, read_trace};

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
