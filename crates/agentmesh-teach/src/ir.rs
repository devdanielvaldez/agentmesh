//! Workflow IR schema, parsing, and static validation.
//!
//! The IR describes _what_ a learned capability does — never _how_ a
//! recorder captured it. Coordinates, screenshots, and raw events belong in
//! recordings; the IR only keeps semantic targets (`role`, `semantic`,
//! accessible names, selector fallbacks) plus `{{ inputs.* }}` and
//! `{{ steps.* }}` templates.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::TeachError;

/// IR format version accepted by this parser.
pub const SUPPORTED_IR_VERSION: &str = "1.0";

/// Operations the validator accepts, grouped by family. Network and vision
/// operations arrive with later milestones; unknown ops fail closed so a
/// workflow can never reference behavior no runtime implements.
pub const KNOWN_OPS: &[&str] = &[
    "browser.navigate",
    "browser.back",
    "browser.forward",
    "browser.new_tab",
    "browser.close_tab",
    "browser.wait_navigation",
    "ui.find",
    "ui.focus",
    "ui.fill",
    "ui.click",
    "ui.activate",
    "ui.press",
    "ui.select",
    "ui.scroll",
    "ui.drag",
    "ui.drop",
    "ui.extract",
    "ui.wait",
    "app.open",
    "app.close",
    "app.focus",
    "file.choose",
    "file.upload",
    "file.download",
    "file.save",
    "auth.ensure_session",
    "auth.request_secret",
    "auth.require_user",
    "control.if",
    "control.switch",
    "control.loop",
    "control.retry",
    "control.timeout",
    "assert.exists",
    "assert.not_exists",
    "assert.text",
    "assert.url",
    "assert.schema",
    "assert.state",
    "human.confirm",
    "human.authenticate",
    "human.resolve",
];

/// A learned capability: inputs, ordered steps, outputs, and its policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    /// IR format version; must be `"1.0"`.
    pub version: String,
    /// Namespaced id (`whatsapp.read_messages`); doubles as the file name.
    pub id: String,
    /// Human-readable description of the capability.
    #[serde(default)]
    pub description: String,
    /// Preferred runtime (`browser`, `desktop`, `mobile`, `api`).
    pub runtime: String,
    /// Declared inputs, referenced as `{{ inputs.<name> }}`.
    #[serde(default)]
    pub inputs: BTreeMap<String, InputDef>,
    /// Ordered steps; later steps may reference earlier results.
    pub steps: Vec<Step>,
    /// Declared outputs, each bound to a step result.
    #[serde(default)]
    pub outputs: BTreeMap<String, OutputDef>,
    /// Execution policy: origins, operations, and rate limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<WorkflowPolicy>,
    /// Conditions that must hold after the entry navigation and before actions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preconditions: Vec<WorkflowAssertion>,
    /// Conditions that must hold after every workflow step has completed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub success: Vec<WorkflowAssertion>,
    /// Conditions that signal an early terminal failure when they become true.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failure: Vec<WorkflowAssertion>,
    /// Retry, observation, checkpoint, and visual-fallback behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<RecoveryPolicy>,
    /// Sanitized API shapes observed during demonstrations. These are hints
    /// for reviewed adapter generation, never executable credentials or bodies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub observed_apis: Vec<ApiObservation>,
}

/// Sanitized network request metadata learned alongside the UI workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiObservation {
    /// HTTP method.
    pub method: String,
    /// Host and optional port, without credentials.
    pub host: String,
    /// URL path, without query values.
    pub path: String,
    /// Query parameter names only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query_keys: Vec<String>,
}

/// One declared workflow input.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputDef {
    /// Value type.
    #[serde(rename = "type")]
    pub input_type: InputType,
    /// Whether the caller must supply the value. Unset means required,
    /// unless a `default` is present (a default implies optional).
    /// Explicit `required: true` together with a default is rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    /// Default value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

impl InputDef {
    /// Resolves whether the caller must supply the value.
    #[must_use]
    pub fn is_required(&self) -> bool {
        match (self.required, &self.default) {
            (Some(required), _) => required,
            (None, Some(_)) => false,
            (None, None) => true,
        }
    }
}

/// Workflow input value types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputType {
    /// UTF-8 text.
    String,
    /// Whole number.
    Integer,
    /// Whole or fractional number.
    Number,
    /// True or false.
    Boolean,
    /// Ordered values.
    Array,
    /// Keyed values.
    Object,
    /// ISO-8601 date or datetime text.
    Datetime,
}

/// One ordered workflow step.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// Step id, unique within the workflow.
    pub id: String,
    /// Operation from [`KNOWN_OPS`].
    pub op: String,
    /// Semantic element the step acts on (required for most `ui.*` ops).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
    /// Templated value (`ui.fill` text, `human.confirm` message, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Navigation URL (`browser.navigate`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Templated result limit (`ui.extract`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<String>,
    /// Step deadline in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Boolean template guard for `control.if` / `control.switch`.
    /// Evaluated against inputs and earlier step results; when it renders
    /// to a falsy value the runtime skips the immediately following step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    /// Templated iteration count for `control.loop`, or a bounded page count
    /// for `ui.scroll`. Scroll also accepts `until_stable`, which stops when
    /// the recorded region no longer moves or loads additional content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iterations: Option<String>,
    /// Drop destination for `ui.drag` / `ui.drop`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<Target>,
    /// File path template for `file.*` operations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Semantic element descriptor. Coordinates are deliberately absent: the
/// resolver tries selectors, then accessibility, then DOM semantics, then
/// structure, then text, then vision, then a human.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// Stable concept name (`conversation_search`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic: Option<String>,
    /// Accessibility role (`textbox`, `button`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Accessible name (`Search`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accessible_name: Option<String>,
    /// Visible text (`Go`, `Cotizaciones`); substring fallback for resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Input placeholder (`Teléfono`); resolved via placeholder lookup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    /// `autocomplete` attribute (`username`, `current-password`); a stable
    /// resolver signal for fields whose ids rotate on every page load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autocomplete: Option<String>,
    /// CSS/XPath fallbacks tried before accessibility resolution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selectors: Vec<String>,
    /// Template matched against candidates (`{{ inputs.contact }}`).
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "match")]
    pub match_pattern: Option<String>,
}

/// One declared workflow output bound to a step result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputDef {
    /// Binding (`steps.read_messages` or `steps.read_messages.result`).
    pub from: String,
}

/// Execution policy carried with the workflow.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowPolicy {
    /// Origins the workflow may drive (`https://web.whatsapp.com`).
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    /// Operations or wildcard families the workflow may use (`ui.*`).
    #[serde(default)]
    pub allowed_operations: Vec<String>,
    /// Operations or wildcard families that always fail closed (`file.*`).
    #[serde(default)]
    pub denied_operations: Vec<String>,
    /// Maximum runs per rolling hour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_runs_per_hour: Option<u64>,
}

/// A state predicate used as a precondition, success verifier, or failure detector.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAssertion {
    /// Supported predicates: `assert.exists`, `assert.not_exists`, `assert.text`, `assert.url`.
    pub op: String,
    /// Semantic UI target for element predicates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
    /// Expected text or URL fragment; input templates are supported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Alternative URL fragment for `assert.url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Predicate deadline in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Runtime recovery and observation controls.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryPolicy {
    /// Maximum attempts for retryable resolution/runtime failures (1–5).
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    /// Step ids after which the current URL becomes a rollback checkpoint.
    #[serde(default)]
    pub checkpoints: Vec<String>,
    /// Persist ARIA snapshots before and after steps.
    #[serde(default = "default_true")]
    pub capture_aria: bool,
    /// Persist a screenshot when the run fails.
    #[serde(default = "default_true")]
    pub capture_screenshot: bool,
    /// Named external visual-repair adapter; the runtime emits a request artifact for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision_adapter: Option<String>,
}

const fn default_max_attempts() -> u32 {
    2
}

const fn default_true() -> bool {
    true
}

/// Parses and validates a workflow document.
///
/// # Errors
///
/// Returns [`TeachError::Parse`] for malformed YAML and
/// [`TeachError::Validation`] when static checks fail.
pub fn parse_workflow(document: &str) -> Result<Workflow, TeachError> {
    let workflow: Workflow = serde_yaml::from_str(document)?;
    validate_workflow(&workflow)?;
    Ok(workflow)
}

/// Runs every static check over an already-parsed workflow.
///
/// # Errors
///
/// Returns [`TeachError::Validation`] describing the first failure.
pub fn validate_workflow(workflow: &Workflow) -> Result<(), TeachError> {
    invalid(|()| {
        if workflow.version != SUPPORTED_IR_VERSION {
            return Err(format!(
                "unsupported version {:?}; expected {SUPPORTED_IR_VERSION:?}",
                workflow.version
            ));
        }
        if !is_workflow_id(&workflow.id) {
            return Err(format!(
                "invalid id {:?}; use lowercase namespace.name segments",
                workflow.id
            ));
        }
        if !is_runtime(&workflow.runtime) {
            return Err(format!(
                "invalid runtime {:?}; expected browser, desktop, mobile, or api",
                workflow.runtime
            ));
        }
        for (name, input) in &workflow.inputs {
            validate_input(name, input)?;
        }
        if workflow.steps.is_empty() {
            return Err("the workflow declares no steps".to_string());
        }
        let mut seen_steps = BTreeSet::new();
        for step in &workflow.steps {
            if !is_step_id(&step.id) {
                return Err(format!(
                    "invalid step id {:?}; use lowercase letters, digits, and underscores",
                    step.id
                ));
            }
            if !seen_steps.insert(step.id.clone()) {
                return Err(format!("duplicate step id {:?}", step.id));
            }
            validate_step(step, &workflow.inputs, &seen_steps)?;
        }
        for (name, output) in &workflow.outputs {
            validate_output(name, output, &seen_steps)?;
        }
        if let Some(policy) = &workflow.policy {
            validate_policy(policy)?;
        }
        for assertion in &workflow.preconditions {
            validate_assertion("precondition", assertion, &workflow.inputs, &seen_steps)?;
        }
        for assertion in &workflow.success {
            validate_assertion("success verifier", assertion, &workflow.inputs, &seen_steps)?;
        }
        for assertion in &workflow.failure {
            validate_assertion("failure detector", assertion, &workflow.inputs, &seen_steps)?;
        }
        if let Some(recovery) = &workflow.recovery {
            validate_recovery(recovery, &seen_steps)?;
        }
        for api in &workflow.observed_apis {
            if api.method.is_empty()
                || !api
                    .method
                    .chars()
                    .all(|character| character.is_ascii_uppercase() || character == '-')
            {
                return Err(format!("observed API has invalid method {:?}", api.method));
            }
            if api.host.is_empty()
                || api.host.contains('@')
                || api.host.contains('/')
                || api.host.chars().any(char::is_whitespace)
            {
                return Err(format!("observed API has unsafe host {:?}", api.host));
            }
            if !api.path.starts_with('/') || api.path.contains('?') || api.path.contains('#') {
                return Err(format!("observed API has unsafe path {:?}", api.path));
            }
        }
        Ok(())
    })
}

fn validate_assertion(
    label: &str,
    assertion: &WorkflowAssertion,
    inputs: &BTreeMap<String, InputDef>,
    seen: &BTreeSet<String>,
) -> Result<(), String> {
    if !matches!(
        assertion.op.as_str(),
        "assert.exists" | "assert.not_exists" | "assert.text" | "assert.url"
    ) {
        return Err(format!(
            "{label} uses unsupported predicate {:?}",
            assertion.op
        ));
    }
    if matches!(assertion.op.as_str(), "assert.exists" | "assert.not_exists") {
        let target = assertion
            .target
            .as_ref()
            .ok_or_else(|| format!("{label} ({}) needs a target", assertion.op))?;
        validate_target(label, target)?;
    }
    if assertion.op == "assert.url"
        && assertion.value.as_deref().is_none_or(str::is_empty)
        && assertion.url.as_deref().is_none_or(str::is_empty)
    {
        return Err(format!("{label} (assert.url) needs value or url"));
    }
    if assertion.op == "assert.text" && assertion.value.as_deref().is_none_or(str::is_empty) {
        return Err(format!("{label} (assert.text) needs value"));
    }
    for template in [&assertion.value, &assertion.url].into_iter().flatten() {
        validate_template(label, template, inputs, seen)?;
    }
    Ok(())
}

fn validate_recovery(recovery: &RecoveryPolicy, steps: &BTreeSet<String>) -> Result<(), String> {
    if !(1..=5).contains(&recovery.max_attempts) {
        return Err("recovery max_attempts must be between 1 and 5".to_string());
    }
    for checkpoint in &recovery.checkpoints {
        if !steps.contains(checkpoint) {
            return Err(format!(
                "recovery references unknown checkpoint {checkpoint:?}"
            ));
        }
    }
    if recovery
        .vision_adapter
        .as_deref()
        .is_some_and(str::is_empty)
    {
        return Err("recovery vision_adapter cannot be empty".to_string());
    }
    Ok(())
}

/// Generalizes brittle recorded assertions so a workflow survives runs beyond
/// the demonstration: `assert.url` keeps scheme, host, and path while query
/// strings, fragments, and a trailing slash are dropped (recorded tracking
/// and highlight parameters never identify a screen), and an `assert.url`
/// that repeats the previous surviving assertion is removed. Templated URLs
/// are left untouched. Returns a human-readable description per change;
/// running it twice is a no-op the second time.
#[must_use]
pub fn relax_workflow(workflow: &mut Workflow) -> Vec<String> {
    let mut changes = Vec::new();
    for step in &mut workflow.steps {
        if step.op != "assert.url" {
            continue;
        }
        let Some(url) = step.url.clone() else {
            continue;
        };
        if url.contains("{{") {
            continue;
        }
        let relaxed = relax_assert_url(&url);
        if relaxed != url {
            changes.push(format!(
                "step {}: assert.url generalized to {relaxed}",
                step.id
            ));
            step.url = Some(relaxed);
        }
    }
    let mut kept: Vec<Step> = Vec::with_capacity(workflow.steps.len());
    for step in workflow.steps.drain(..) {
        let duplicate = step.op == "assert.url"
            && kept.last().is_some_and(|previous: &Step| {
                previous.op == "assert.url"
                    && previous.url == step.url
                    && previous.value == step.value
                    && previous.timeout_ms == step.timeout_ms
            });
        if duplicate {
            changes.push(format!(
                "step {}: removed as a duplicate of the previous assertion",
                step.id
            ));
            continue;
        }
        kept.push(step);
    }
    workflow.steps = kept;
    changes
}

/// Drops the query string, fragment, and trailing slash of a recorded URL,
/// keeping at least `scheme://host` so the result still matches its screen.
fn relax_assert_url(url: &str) -> String {
    let base = url.split(['?', '#']).next().unwrap_or(url);
    let trimmed = base.trim_end_matches('/');
    if let Some(scheme_end) = base.find("://").map(|index| index + 3) {
        if trimmed.len() > scheme_end {
            return strip_opaque_tail(scheme_end, trimmed);
        }
        // Host-only URL: canonicalize to `scheme://host`.
        return base[..scheme_end].to_string() + base[scheme_end..].trim_matches('/');
    }
    if trimmed.is_empty() {
        return base.to_string();
    }
    trimmed.to_string()
}

/// Strips trailing path segments that look like opaque entity ids (long,
/// carrying digits, uppercase, or escapes) instead of screen names: screen
/// names are short lowercase slugs, so `.../thread/2-AbC...` relaxes to
/// `.../thread` while `.../notifications` is untouched.
fn strip_opaque_tail(scheme_end: usize, url: &str) -> String {
    let mut relaxed = url.to_string();
    while let Some(slash) = relaxed.rfind('/') {
        if slash < scheme_end {
            break;
        }
        let tail = &relaxed[slash + 1..];
        if !is_opaque_id(tail) {
            break;
        }
        relaxed.truncate(slash);
    }
    if relaxed.len() <= scheme_end {
        return url.trim_end_matches('/').to_string();
    }
    relaxed
}

/// Heuristic for recorded entity ids: long segments that are not plain
/// lowercase slugs (they carry digits, uppercase, or URL escapes).
fn is_opaque_id(segment: &str) -> bool {
    segment.len() >= 12
        && segment.chars().any(|character| {
            character.is_ascii_digit() || character.is_ascii_uppercase() || character == '%'
        })
}

/// Extracts `{{ ... }}` template references from a string, trimmed and
/// non-empty. Malformed spans (unclosed braces) are ignored so validation
/// reports them as literal text, never as references.
#[must_use]
pub fn extract_template_refs(template: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            break;
        };
        let reference = after[..close].trim();
        if !reference.is_empty() {
            refs.push(reference.to_string());
        }
        rest = &after[close + 2..];
    }
    refs
}

/// Validates one input definition, including its default value type.
fn validate_input(name: &str, input: &InputDef) -> Result<(), String> {
    if !is_name(name) {
        return Err(format!("invalid input name {name:?}"));
    }
    if let Some(default) = &input.default {
        if input.required == Some(true) {
            return Err(format!("input {name:?} sets both required and a default"));
        }
        if !default_matches(input.input_type, default) {
            return Err(format!(
                "input {name:?} default does not match {:?}",
                type_name(input.input_type)
            ));
        }
    }
    Ok(())
}

/// Validates one step against known ops, target rules, and template refs.
/// `seen` holds the ids of steps declared before this one.
fn validate_step(
    step: &Step,
    inputs: &BTreeMap<String, InputDef>,
    seen: &BTreeSet<String>,
) -> Result<(), String> {
    if !KNOWN_OPS.contains(&step.op.as_str()) {
        return Err(format!("step {:?} uses unknown op {:?}", step.id, step.op));
    }
    if step.op == "browser.navigate" && step.url.as_deref().is_none_or(str::is_empty) {
        return Err(format!("step {:?} (browser.navigate) needs a url", step.id));
    }
    if needs_target(&step.op) {
        let Some(target) = &step.target else {
            return Err(format!("step {:?} ({}) needs a target", step.id, step.op));
        };
        validate_target(&step.id, target)?;
    } else if let Some(target) = &step.target {
        validate_target(&step.id, target)?;
    }
    if let Some(destination) = &step.destination {
        validate_target(&step.id, destination)?;
    }
    if matches!(step.op.as_str(), "control.if" | "control.switch")
        && step.condition.as_deref().is_none_or(str::is_empty)
    {
        return Err(format!(
            "step {:?} ({}) needs a condition template",
            step.id, step.op
        ));
    }
    if step.op == "control.loop" && step.iterations.as_deref().is_none_or(str::is_empty) {
        return Err(format!(
            "step {:?} (control.loop) needs an iterations template",
            step.id
        ));
    }
    if step.op == "ui.scroll" {
        if step
            .value
            .as_deref()
            .is_some_and(|value| !matches!(value, "up" | "down" | "left" | "right"))
        {
            return Err(format!(
                "step {:?} (ui.scroll) direction must be up, down, left, or right",
                step.id
            ));
        }
        if let Some(iterations) = step
            .iterations
            .as_deref()
            .filter(|value| !value.contains("{{"))
        {
            let valid = iterations == "until_stable"
                || iterations
                    .parse::<u32>()
                    .is_ok_and(|count| (1..=100).contains(&count));
            if !valid {
                return Err(format!(
                    "step {:?} (ui.scroll) iterations must be 1-100 or until_stable",
                    step.id
                ));
            }
        }
    }
    if step.op.starts_with("file.")
        && step.path.as_deref().is_none_or(str::is_empty)
        && step.value.as_deref().is_none_or(str::is_empty)
    {
        return Err(format!(
            "step {:?} ({}) needs a path or value template",
            step.id, step.op
        ));
    }
    for template in [
        &step.value,
        &step.limit,
        &step.condition,
        &step.iterations,
        &step.path,
    ]
    .into_iter()
    .flatten()
    {
        validate_template(&step.id, template, inputs, seen)?;
    }
    if let Some(url) = &step.url {
        validate_template(&step.id, url, inputs, seen)?;
    }
    Ok(())
}

/// Whether the op family addresses a UI element.
fn needs_target(op: &str) -> bool {
    op.starts_with("ui.") && op != "ui.wait"
}

/// Validates that a target carries at least one resolution signal.
fn validate_target(step_id: &str, target: &Target) -> Result<(), String> {
    if target.semantic.as_deref().is_none_or(str::is_empty)
        && target.role.as_deref().is_none_or(str::is_empty)
        && target.accessible_name.as_deref().is_none_or(str::is_empty)
        && target.text.as_deref().is_none_or(str::is_empty)
        && target.placeholder.as_deref().is_none_or(str::is_empty)
        && target.match_pattern.as_deref().is_none_or(str::is_empty)
        && target.selectors.iter().all(String::is_empty)
    {
        return Err(format!(
            "step {step_id:?} target needs a semantic, role, accessible_name, text, placeholder, match, or selector"
        ));
    }
    Ok(())
}

/// Validates every `{{ }}` reference in a template: inputs must be declared,
/// steps must exist and precede the referencing step.
fn validate_template(
    step_id: &str,
    template: &str,
    inputs: &BTreeMap<String, InputDef>,
    seen: &BTreeSet<String>,
) -> Result<(), String> {
    for reference in extract_template_refs(template) {
        let mut segments = reference.split('.');
        match (segments.next(), segments.next(), segments.next()) {
            (Some("inputs"), Some(name), None) => {
                if !inputs.contains_key(name) {
                    return Err(format!(
                        "step {step_id:?} references unknown input {name:?}"
                    ));
                }
            }
            (Some("steps"), Some(id), _) => {
                if !seen.contains(id) {
                    return Err(format!(
                        "step {step_id:?} references unknown or later step {id:?}"
                    ));
                }
            }
            _ => {
                return Err(format!(
                    "step {step_id:?} has an invalid reference {{{{{reference}}}}}; \
                     use {{{{ inputs.<name> }}}} or {{{{ steps.<id>[.result] }}}}"
                ));
            }
        }
    }
    Ok(())
}

/// Validates an output binding against previously declared steps.
fn validate_output(name: &str, output: &OutputDef, seen: &BTreeSet<String>) -> Result<(), String> {
    if !is_name(name) {
        return Err(format!("invalid output name {name:?}"));
    }
    let mut segments = output.from.split('.');
    match (segments.next(), segments.next(), segments.next()) {
        (Some("steps"), Some(id), None | Some("result")) if seen.contains(id) => Ok(()),
        _ => Err(format!(
            "output {name:?} binds to {:?}; use steps.<id>[.result] of a declared step",
            output.from
        )),
    }
}

/// Validates workflow policy bounds (origins, rate limit).
fn validate_policy(policy: &WorkflowPolicy) -> Result<(), String> {
    for origin in &policy.allowed_origins {
        if !(origin.starts_with("https://") || origin.starts_with("http://")) {
            return Err(format!("allowed origin {origin:?} must be an http(s) URL"));
        }
    }
    if policy.max_runs_per_hour.is_some_and(|limit| limit == 0) {
        return Err("max_runs_per_hour must be greater than zero".to_string());
    }
    for operation in policy
        .allowed_operations
        .iter()
        .chain(&policy.denied_operations)
    {
        let valid = KNOWN_OPS.contains(&operation.as_str())
            || operation.strip_suffix(".*").is_some_and(|family| {
                KNOWN_OPS
                    .iter()
                    .any(|known| known.starts_with(&format!("{family}.")))
            });
        if !valid {
            return Err(format!("policy references unknown operation {operation:?}"));
        }
    }
    Ok(())
}

/// Maps a validation closure error into [`TeachError::Validation`].
fn invalid(check: impl FnOnce(()) -> Result<(), String>) -> Result<(), TeachError> {
    check(()).map_err(TeachError::Validation)
}

/// Workflow ids are dot-separated lowercase segments (`app.capability`).
fn is_workflow_id(id: &str) -> bool {
    let mut segments = id.split('.');
    let Some(first) = segments.next() else {
        return false;
    };
    if !is_name(first) {
        return false;
    }
    let mut dots = 0;
    for segment in segments {
        if !is_name(segment) {
            return false;
        }
        dots += 1;
    }
    dots >= 1
}

/// Step, input, and output names: lowercase start, then alnum/underscore.
fn is_name(name: &str) -> bool {
    is_step_id(name)
}

/// Step ids: lowercase start, then alnum/underscore.
fn is_step_id(id: &str) -> bool {
    let mut chars = id.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => (),
        _ => return false,
    }
    chars.all(|char| char.is_ascii_lowercase() || char.is_ascii_digit() || char == '_')
}

/// Supported preferred runtimes.
fn is_runtime(runtime: &str) -> bool {
    matches!(runtime, "browser" | "desktop" | "mobile" | "api")
}

/// Human type name for error messages.
fn type_name(input_type: InputType) -> &'static str {
    match input_type {
        InputType::String | InputType::Datetime => "string",
        InputType::Integer => "integer",
        InputType::Number => "number",
        InputType::Boolean => "boolean",
        InputType::Array => "array",
        InputType::Object => "object",
    }
}

/// Checks a default value against its declared input type.
fn default_matches(input_type: InputType, default: &Value) -> bool {
    match (input_type, default) {
        (InputType::Integer, Value::Number(number)) => number.is_i64() || number.is_u64(),
        (InputType::String | InputType::Datetime, Value::String(_))
        | (InputType::Number, Value::Number(_))
        | (InputType::Boolean, Value::Bool(_))
        | (InputType::Array, Value::Array(_))
        | (InputType::Object, Value::Object(_)) => true,
        _ => false,
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    const WHATSAPP: &str = r#"
version: "1.0"
id: whatsapp.read_messages
description: Read recent WhatsApp messages
runtime: browser
inputs:
  contact:
    type: string
    required: true
  limit:
    type: integer
    default: 20
steps:
  - id: open_whatsapp
    op: browser.navigate
    url: https://web.whatsapp.com
  - id: search_contact
    op: ui.fill
    target:
      semantic: conversation_search
      role: textbox
    value: "{{ inputs.contact }}"
  - id: select_contact
    op: ui.activate
    target:
      semantic: conversation_result
      match: "{{ inputs.contact }}"
  - id: read_messages
    op: ui.extract
    target:
      semantic: message_list
    limit: "{{ inputs.limit }}"
outputs:
  messages:
    from: steps.read_messages.result
policy:
  allowed_origins:
    - https://web.whatsapp.com
  allowed_operations:
    - browser.*
    - ui.*
  denied_operations:
    - file.*
  max_runs_per_hour: 60
"#;

    #[test]
    fn spec_example_parses_and_validates() {
        let workflow = parse_workflow(WHATSAPP).expect("spec example is valid");
        assert_eq!(workflow.id, "whatsapp.read_messages");
        assert_eq!(workflow.steps.len(), 4);
        assert!(workflow.outputs.contains_key("messages"));
    }

    #[test]
    fn text_only_target_validates() {
        let yaml = "version: \"1.0\"\nid: app.go\nruntime: browser\nsteps:\n  - id: go\n    op: ui.click\n    target:\n      text: Go\n";
        let workflow = parse_workflow(yaml).expect("text-only target parses");
        validate_workflow(&workflow).expect("text-only target validates");
    }

    #[test]
    fn placeholder_only_target_validates() {
        let yaml = "version: \"1.0\"\nid: app.call\nruntime: browser\nsteps:\n  - id: dial\n    op: ui.fill\n    target:\n      role: textbox\n      placeholder: Teléfono\n    value: '809'\n";
        let workflow = parse_workflow(yaml).expect("placeholder target parses");
        validate_workflow(&workflow).expect("placeholder target validates");
    }

    #[test]
    fn relax_generalizes_urls_and_dedupes_asserts() {
        let yaml = "version: \"1.0\"\nid: app.read\nruntime: browser\ninputs:\n  q:\n    type: string\n    required: true\nsteps:\n  - id: open\n    op: browser.navigate\n    url: https://app.example/\n  - id: first\n    op: assert.url\n    url: https://app.example/items/?filter=all#top\n  - id: second\n    op: assert.url\n    url: https://app.example/items\n  - id: templated\n    op: assert.url\n    url: 'https://app.example/search?q={{ inputs.q }}'\n  - id: thread\n    op: assert.url\n    url: https://app.example/thread/2-NjgzMjY3M2EtN2VhNS00M2ViLThmZDctNjMwZjFiOWMyYTVlXzEwMA==\n";
        let mut workflow = parse_workflow(yaml).expect("relax fixture parses");
        let changes = relax_workflow(&mut workflow);
        assert_eq!(
            changes.len(),
            3,
            "two generalizes plus one duplicate: {changes:?}"
        );
        let urls: Vec<_> = workflow
            .steps
            .iter()
            .filter(|step| step.op == "assert.url")
            .map(|step| step.url.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            urls,
            vec![
                "https://app.example/items".to_string(),
                "https://app.example/search?q={{ inputs.q }}".to_string(),
                "https://app.example/thread".to_string()
            ]
        );
        validate_workflow(&workflow).expect("relaxed workflow still validates");
        assert!(
            relax_workflow(&mut workflow).is_empty(),
            "relax is idempotent"
        );
    }

    #[test]
    fn template_refs_extracted_and_malformed_ignored() {
        assert_eq!(
            extract_template_refs("to {{ inputs.contact }} via {{steps.a.result}}"),
            vec!["inputs.contact", "steps.a.result"]
        );
        assert!(extract_template_refs("no refs").is_empty());
        assert!(extract_template_refs("unclosed {{ inputs.x").is_empty());
    }

    fn case(document: &str) -> String {
        parse_workflow(&format!(
            "version: \"1.0\"\nid: app.case\nruntime: browser\n{document}"
        ))
        .expect_err("case is invalid")
        .to_string()
    }

    #[test]
    fn duplicate_step_ids_rejected() {
        let message = case("steps:\n  - id: a\n    op: ui.wait\n  - id: a\n    op: ui.wait\n");
        assert!(message.contains("duplicate step id"), "{message}");
    }

    #[test]
    fn unknown_op_rejected() {
        let message = case("steps:\n  - id: a\n    op: ui.teleport\n");
        assert!(message.contains("unknown op"), "{message}");
    }

    #[test]
    fn ui_fill_without_target_rejected() {
        let message = case("steps:\n  - id: a\n    op: ui.fill\n    value: hi\n");
        assert!(message.contains("needs a target"), "{message}");
    }

    #[test]
    fn unknown_input_ref_rejected() {
        let message = case(
            "steps:\n  - id: a\n    op: ui.fill\n    target:\n      role: textbox\n    value: 'fill {{ inputs.missing }} now'",
        );
        assert!(message.contains("unknown input"), "{message}");
    }

    #[test]
    fn forward_step_ref_rejected() {
        let message = case(
            "steps:\n  - id: a\n    op: ui.fill\n    target:\n      role: textbox\n    value: 'use {{ steps.b.result }}'\n  - id: b\n    op: ui.wait\n",
        );
        assert!(message.contains("later step"), "{message}");
    }

    #[test]
    fn output_binding_unknown_step_rejected() {
        let message = case(
            "steps:\n  - id: a\n    op: ui.wait\noutputs:\n  out:\n    from: steps.ghost.result\n",
        );
        assert!(message.contains("steps.<id>"), "{message}");
    }

    #[test]
    fn ids_and_version_rejected() {
        let workflow: Workflow =
            serde_yaml::from_str("version: \"2.0\"\nid: app.x\nruntime: browser\nsteps: []")
                .expect("version 2.0 parses");
        let message = validate_workflow(&workflow)
            .expect_err("version 2.0 unsupported")
            .to_string();
        assert!(message.contains("unsupported version"), "{message}");
        let message = parse_workflow("version: \"1.0\"\nid: nodots\nruntime: browser\nsteps: []")
            .expect_err("id without namespace")
            .to_string();
        assert!(message.contains("invalid id"), "{message}");
    }

    #[test]
    fn input_defaults_checked() {
        let message = case(
            "inputs:\n  x:\n    type: string\n    required: true\n    default: hi\nsteps:\n  - id: a\n    op: ui.wait\n",
        );
        assert!(message.contains("required and a default"), "{message}");
        let message = case(
            "inputs:\n  x:\n    type: integer\n    required: false\n    default: many\nsteps:\n  - id: a\n    op: ui.wait\n",
        );
        assert!(message.contains("does not match"), "{message}");
    }

    #[test]
    fn policy_bounds_checked() {
        let message = case(
            "steps:\n  - id: a\n    op: ui.wait\npolicy:\n  allowed_origins:\n    - evil.com\n",
        );
        assert!(message.contains("http(s)"), "{message}");
        let message = case("steps:\n  - id: a\n    op: ui.wait\npolicy:\n  max_runs_per_hour: 0\n");
        assert!(message.contains("greater than zero"), "{message}");
    }

    #[test]
    fn policy_operations_accept_exact_and_family_wildcards() {
        parse_workflow(
            "version: \"1.0\"\nid: app.case\nruntime: browser\nsteps:\n  - id: a\n    op: ui.wait\npolicy:\n  allowed_operations: [ui.*]\n",
        )
        .expect("known family wildcard is valid");
        let message =
            case("steps:\n  - id: a\n    op: ui.wait\npolicy:\n  denied_operations: [shell.*]\n");
        assert!(message.contains("unknown operation"), "{message}");
    }

    #[test]
    fn empty_steps_rejected() {
        let message = case("steps: []\n");
        assert!(message.contains("no steps"), "{message}");
    }

    #[test]
    fn recovery_and_state_contracts_validate() {
        parse_workflow(
            "version: \"1.0\"\nid: app.case\nruntime: browser\nsteps:\n  - id: open\n    op: browser.navigate\n    url: https://example.com\npreconditions:\n  - op: assert.url\n    value: example.com\nsuccess:\n  - op: assert.exists\n    target: { role: main }\nrecovery:\n  max_attempts: 3\n  checkpoints: [open]\n",
        )
        .expect("state contracts and recovery are valid");
        let message = case(
            "steps:\n  - id: open\n    op: ui.wait\nrecovery:\n  max_attempts: 6\n  checkpoints: [missing]\n",
        );
        assert!(message.contains("max_attempts"), "{message}");
    }

    #[test]
    fn observed_api_metadata_rejects_credentials_and_query_values() {
        let message = case(
            "steps:\n  - id: a\n    op: ui.wait\nobserved_apis:\n  - method: GET\n    host: user:password@example.com\n    path: /search?q=secret\n",
        );
        assert!(message.contains("unsafe host"), "{message}");
        parse_workflow(
            "version: \"1.0\"\nid: app.api\nruntime: browser\nsteps:\n  - id: a\n    op: ui.wait\nobserved_apis:\n  - method: POST\n    host: api.example.com\n    path: /v1/search\n    query_keys: [q]\n",
        )
        .expect("sanitized API observation is valid");
    }

    #[test]
    fn control_and_file_step_contracts_validate() {
        parse_workflow(
            "version: \"1.0\"\nid: app.case\nruntime: browser\ninputs:\n  go:\n    type: string\n    required: false\nsteps:\n  - id: gate\n    op: control.if\n    condition: '{{ inputs.go }}'\n  - id: a\n    op: ui.wait\n  - id: repeat\n    op: control.loop\n    iterations: '3'\n  - id: b\n    op: ui.wait\n  - id: save\n    op: file.save\n    path: /tmp/out.txt\n    value: hello\n",
        )
        .expect("control and file contracts are valid");
        let message = case("steps:\n  - id: gate\n    op: control.if\n");
        assert!(message.contains("condition"), "{message}");
        let message = case("steps:\n  - id: repeat\n    op: control.loop\n");
        assert!(message.contains("iterations"), "{message}");
        let message = case("steps:\n  - id: save\n    op: file.save\n");
        assert!(message.contains("path or value"), "{message}");
    }

    #[test]
    fn scroll_pagination_contract_is_bounded() {
        parse_workflow(
            "version: \"1.0\"\nid: app.feed\nruntime: browser\nsteps:\n  - id: paginate\n    op: ui.scroll\n    target: { role: feed, accessible_name: Messages }\n    value: down\n    iterations: until_stable\n",
        )
        .expect("bounded infinite-scroll pagination is valid");
        let message = case(
            "steps:\n  - id: paginate\n    op: ui.scroll\n    target: { role: feed }\n    value: diagonal\n    iterations: '101'\n",
        );
        assert!(message.contains("direction"), "{message}");
        let message = case(
            "steps:\n  - id: paginate\n    op: ui.scroll\n    target: { role: feed }\n    value: down\n    iterations: '101'\n",
        );
        assert!(message.contains("1-100"), "{message}");
    }
}
