//! Local lifecycle commands for reusable MCP composite capabilities.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::IsTerminal,
    path::{Path, PathBuf},
};

use agentmesh_protocol::{CapabilityPackage, EffectKind};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{CompositeAction, teach_flow};

const DEFAULT_SERVER_NAME: &str = "agentmesh-taught-capabilities";
const MAX_COMPOSITE_STEPS: usize = 12;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompositeRecipe {
    id: String,
    description: String,
    input_schema: Value,
    steps: Vec<CompositeStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompositeStep {
    step_id: String,
    capability_id: String,
    input_mapping: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompositeRecord {
    id: String,
    tool: String,
    description: String,
    runtime: String,
    is_composite: bool,
    steps: Vec<CompositeStep>,
    step_count: usize,
    has_write: bool,
    inputs: Value,
    outputs: Value,
    effects: Vec<String>,
    risk: String,
    permissions: Vec<String>,
    success_checks: Vec<Value>,
    workflow_summary: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    package_digest: Option<String>,
    contract: CapabilityPackage,
}

#[derive(Debug, Clone)]
pub(crate) struct ConsoleComposite {
    pub id: String,
    pub description: String,
    pub step_count: usize,
    pub risk: String,
    pub has_write: bool,
    pub inputs: Value,
}

pub(crate) fn run(action: CompositeAction, state_dir: Option<&Path>) -> Result<()> {
    let directory = resolve_state_dir(state_dir)?;
    let file = directory.join("composites.json");
    match action {
        CompositeAction::List => list(&file),
        CompositeAction::Inspect { id } => inspect(&file, &id),
        CompositeAction::Create { recipe } => create(&file, &recipe),
        CompositeAction::Run {
            id,
            input,
            yes,
            session,
            headed,
        } => execute(&file, &id, &input, yes, session.as_deref(), headed),
    }
}

pub(crate) fn console_catalog() -> Result<Vec<ConsoleComposite>> {
    let path = resolve_state_dir(None)?.join("composites.json");
    Ok(read_records(&path)?
        .into_iter()
        .map(|record| ConsoleComposite {
            id: record.id,
            description: record.description,
            step_count: record.step_count,
            risk: record.risk,
            has_write: record.has_write,
            inputs: record.inputs,
        })
        .collect())
}

fn resolve_state_dir(override_dir: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = override_dir {
        return Ok(path.to_path_buf());
    }
    let home = agentmesh_teach::home_dir().context("no AgentMesh data home; set AGENTMESH_HOME")?;
    Ok(home.join("mcp-state").join(DEFAULT_SERVER_NAME))
}

fn read_records(path: &Path) -> Result<Vec<CompositeRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes =
        std::fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid composite catalog {}", path.display()))
}

fn list(path: &Path) -> Result<()> {
    let records = read_records(path)?;
    if records.is_empty() {
        println!(
            "No composite tools saved. Create one with `agentmesh composites create recipe.json`."
        );
        return Ok(());
    }
    println!("{:<32} {:>5}  {:<8} DESCRIPTION", "ID", "STEPS", "RISK");
    for record in records {
        println!(
            "{:<32} {:>5}  {:<8} {}",
            record.id, record.step_count, record.risk, record.description
        );
    }
    Ok(())
}

fn inspect(path: &Path, id: &str) -> Result<()> {
    let record = read_records(path)?
        .into_iter()
        .find(|record| record.id == id || record.id == qualify_id(id))
        .with_context(|| format!("composite {id:?} was not found"))?;
    println!("{}", serde_json::to_string_pretty(&record)?);
    Ok(())
}

fn create(path: &Path, recipe_path: &Path) -> Result<()> {
    let text = std::fs::read_to_string(recipe_path)
        .with_context(|| format!("could not read recipe {}", recipe_path.display()))?;
    let recipe: CompositeRecipe = match recipe_path.extension().and_then(|value| value.to_str()) {
        Some("yaml" | "yml") => {
            serde_yaml::from_str(&text).context("invalid composite YAML recipe")?
        }
        _ => serde_json::from_str(&text).context("invalid composite JSON recipe")?,
    };
    let records = read_records(path)?;
    let record = compile_recipe(recipe, &records)?;
    let mut records = records;
    records.push(record.clone());
    write_records(path, &records)?;
    println!("Created {} as MCP tool `{}`.", record.id, record.tool);
    println!("State: {}", path.display());
    println!("Reload the running MCP server with `teach_reload_capabilities` or restart it.");
    Ok(())
}

fn write_records(path: &Path, records: &[CompositeRecord]) -> Result<()> {
    let parent = path
        .parent()
        .context("composite catalog has no parent directory")?;
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(records)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&temporary, path)
        .with_context(|| format!("could not update {}", path.display()))
}

fn compile_recipe(
    recipe: CompositeRecipe,
    existing: &[CompositeRecord],
) -> Result<CompositeRecord> {
    let id = qualify_id(&recipe.id);
    if !valid_namespaced_id(&id) {
        bail!(
            "use a namespaced id with lowercase segments, for example `composite.customer_follow_up`"
        );
    }
    let tool = id.replace('.', "_");
    if existing
        .iter()
        .any(|record| record.id == id || record.tool == tool)
    {
        bail!("composite id or MCP tool name already exists: {id}");
    }
    if agentmesh_teach::list_workflows()?
        .iter()
        .any(|workflow| workflow.id == id || workflow.id.replace('.', "_") == tool)
    {
        bail!("composite id or MCP tool name conflicts with a stored workflow: {id}");
    }
    if recipe.description.trim().is_empty() || recipe.description.len() > 1_000 {
        bail!("description must contain 1–1000 characters");
    }
    let mut input_schema = recipe.input_schema.clone();
    validate_input_schema(&input_schema)?;
    input_schema["additionalProperties"] = Value::Bool(false);
    if recipe.steps.is_empty() || recipe.steps.len() > MAX_COMPOSITE_STEPS {
        bail!("a composite needs between 1 and {MAX_COMPOSITE_STEPS} steps");
    }

    let mut prior_outputs = BTreeMap::<String, Value>::new();
    let mut effects = BTreeSet::new();
    let mut permissions = BTreeSet::new();
    let mut claims = Vec::new();
    let mut summary = Vec::new();
    let mut has_write = false;
    let mut seen_steps = BTreeSet::new();

    for step in &recipe.steps {
        if !valid_step_id(&step.step_id) || !seen_steps.insert(step.step_id.clone()) {
            bail!(
                "step ids must be unique lowercase names; invalid id {:?}",
                step.step_id
            );
        }
        let workflow = agentmesh_teach::load_workflow(&step.capability_id)
            .with_context(|| format!("could not load step capability {}", step.capability_id))?;
        let package =
            agentmesh_teach::compile_workflow_capability(&workflow).with_context(|| {
                format!(
                    "{} needs saved success checks before it can be composed",
                    step.capability_id
                )
            })?;
        validate_step_mapping(step, &workflow, &input_schema, &prior_outputs)?;
        prior_outputs.insert(step.step_id.clone(), package.contract.outputs.clone());
        for effect in &package.contract.effects {
            effects.insert(*effect);
        }
        permissions.extend(package.authority.permissions.iter().cloned());
        has_write |= teach_flow::workflow_has_write(&workflow);
        claims.extend(package.contract.success_evidence.iter().map(|claim| {
            let mut claim = claim.clone();
            claim.id = format!("{}.{}", step.step_id, claim.id);
            claim
        }));
        summary.push(json!({
            "id": step.step_id,
            "action": format!("execute {}", step.capability_id),
        }));
    }

    let risk = risk_for(&effects);
    let output_schema = json!({
        "type": "object",
        "properties": { "steps": { "type": "object" } },
        "required": ["steps"],
        "additionalProperties": false
    });
    let mut contract = serde_json::to_value(agentmesh_teach::compile_workflow_capability(
        &agentmesh_teach::load_workflow(&recipe.steps[0].capability_id)?,
    )?)?;
    let package = contract
        .as_object_mut()
        .context("generated package is not an object")?;
    package.insert(
        "capability".into(),
        json!({ "id": id, "version": "1.0.0", "intent": recipe.description }),
    );
    let requirements: BTreeSet<_> = recipe
        .steps
        .iter()
        .map(|step| step.capability_id.clone())
        .collect();
    package.remove("x-agentmesh-teach");
    package.insert("x-agentmesh-composition".into(), json!({
        "steps": recipe.steps.iter().map(|step| json!({ "id": step.step_id, "capability": step.capability_id })).collect::<Vec<_>>(),
        "recovery": "reconcile external state after any partial failure"
    }));
    package.insert(
        "contract".into(),
        json!({
            "requires": requirements.iter().map(|id| json!({ "id": id })).collect::<Vec<_>>(),
            "inputs": input_schema,
            "outputs": output_schema,
            "successEvidence": claims,
            "effects": effects,
            "idempotency": "not_guaranteed",
            "recovery": "human_review"
        }),
    );
    package.insert("authority".into(), json!({
        "permissions": permissions,
        "approvals": if has_write { vec![json!({ "before": "execute", "display": input_schema["properties"].as_object().map(|props| props.keys().cloned().collect::<Vec<_>>()).unwrap_or_default() })] } else { Vec::new() },
        "constraints": {}
    }));
    package.insert(
        "implementations".into(),
        json!([{
            "id": "teach-composition",
            "kind": "agentmesh_workflow",
            "reference": format!("composition://{id}"),
            "configuration": {}
        }]),
    );
    package.insert("provenance".into(), Value::Null);
    let contract: CapabilityPackage = serde_json::from_value(contract)?;
    contract
        .validate()
        .context("composite capability contract is invalid")?;
    let package_digest = Some(contract.content_digest()?);

    Ok(CompositeRecord {
        id: id.clone(),
        tool,
        description: recipe.description,
        runtime: "composite".into(),
        is_composite: true,
        step_count: recipe.steps.len(),
        steps: recipe.steps,
        has_write,
        inputs: input_schema,
        outputs: json!({ "steps": { "type": "object" } }),
        effects: effects
            .iter()
            .map(|effect| effect_name(*effect).to_string())
            .collect(),
        risk,
        permissions: permissions.into_iter().collect(),
        success_checks: contract
            .contract
            .success_evidence
            .iter()
            .map(|claim| {
                json!({
                    "id": claim.id,
                    "assertion": claim.assertion,
                })
            })
            .collect(),
        workflow_summary: summary,
        package_digest,
        contract,
    })
}

fn validate_step_mapping(
    step: &CompositeStep,
    workflow: &agentmesh_teach::Workflow,
    input_schema: &Value,
    prior_outputs: &BTreeMap<String, Value>,
) -> Result<()> {
    for (name, input) in &workflow.inputs {
        if input.is_required() && !step.input_mapping.contains_key(name) && input.default.is_none()
        {
            bail!("step {} must map required input {name}", step.step_id);
        }
    }
    for (name, value) in &step.input_mapping {
        if !workflow.inputs.contains_key(name) {
            bail!("step {} maps unknown input {name}", step.step_id);
        }
        if is_credential_name(name) {
            bail!(
                "step {} maps credential-like input {name}; credentials must stay in the workflow secret store",
                step.step_id
            );
        }
        validate_mapping_reference(value, input_schema, prior_outputs)
            .with_context(|| format!("invalid mapping for {}.{name}", step.step_id))?;
    }
    Ok(())
}

fn validate_mapping_reference(
    value: &Value,
    input_schema: &Value,
    prior: &BTreeMap<String, Value>,
) -> Result<()> {
    match value {
        Value::Array(items) => {
            for item in items {
                validate_mapping_reference(item, input_schema, prior)?;
            }
        }
        Value::Object(object)
            if object.len() == 1 && object.get("$input").and_then(Value::as_str).is_some() =>
        {
            let name = object["$input"].as_str().expect("checked as string");
            if is_credential_name(name) {
                bail!("credentials cannot be passed as composite inputs");
            }
            if !input_schema["properties"]
                .as_object()
                .is_some_and(|properties| properties.contains_key(name))
            {
                bail!("unknown composite input {name}");
            }
            let required = input_schema
                .get("required")
                .and_then(Value::as_array)
                .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(name)));
            let has_default = input_schema["properties"][name].get("default").is_some();
            if !required && !has_default {
                bail!(
                    "composite input {name} is optional and has no default; it cannot be mapped safely"
                );
            }
        }
        Value::Object(object)
            if object.len() == 2
                && object.get("$step").and_then(Value::as_str).is_some()
                && object.get("path").and_then(Value::as_str).is_some() =>
        {
            let step_id = object["$step"].as_str().expect("checked as string");
            let path = object["path"].as_str().expect("checked as string");
            let (root, name) = path
                .split_once('.')
                .context("output reference path must be outputs.NAME")?;
            if root != "outputs" || name.is_empty() {
                bail!("output reference path must be outputs.NAME");
            }
            let schema = prior
                .get(step_id)
                .with_context(|| format!("{step_id} must refer to an earlier step"))?;
            if !schema["properties"]
                .as_object()
                .is_some_and(|properties| properties.contains_key(name))
            {
                bail!("step {step_id} has no declared output {name}");
            }
        }
        Value::Object(object) => {
            for item in object.values() {
                validate_mapping_reference(item, input_schema, prior)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn execute(
    path: &Path,
    id: &str,
    raw_inputs: &[String],
    yes: bool,
    session: Option<&str>,
    headed: bool,
) -> Result<()> {
    let record = read_records(path)?
        .into_iter()
        .find(|record| record.id == id || record.id == qualify_id(id))
        .with_context(|| format!("composite {id:?} was not found"))?;
    let mut inputs = Value::Object(parse_inputs(raw_inputs)?);
    apply_schema_defaults(&mut inputs, &record.inputs);
    validate_value(&inputs, &record.inputs, "inputs")?;
    let inputs = inputs
        .as_object()
        .context("composite inputs must be an object")?;
    if record.has_write {
        println!("Plan: {}", record.description);
        println!(
            "Risk: {}  Effects: {}",
            record.risk,
            record.effects.join(", ")
        );
        for step in &record.steps {
            println!(
                "  {} → {} (inputs: {})",
                step.step_id,
                step.capability_id,
                serde_json::to_string(&step.input_mapping)?
            );
        }
    }
    if record.has_write && !yes {
        if !std::io::stdin().is_terminal() {
            bail!(
                "{} has write effects; run in a terminal or pass --yes",
                record.id
            );
        }
        if !crate::teach_yes_no("Execute this complete composition?", false)? {
            bail!("composite execution declined");
        }
    }
    if record.has_write && yes {
        println!("Write effects pre-authorized by --yes.");
    }

    let mut step_results = Map::new();
    let mut completed = Vec::new();
    for step in &record.steps {
        let workflow = agentmesh_teach::load_workflow(&step.capability_id)?;
        let mut mapped = Map::new();
        for (name, value) in &step.input_mapping {
            mapped.insert(name.clone(), resolve_value(value, &inputs, &step_results)?);
        }
        for (name, input) in &workflow.inputs {
            if !mapped.contains_key(name) {
                if let Some(default) = &input.default {
                    mapped.insert(name.clone(), default.clone());
                }
            }
        }
        if let Err(error) = validate_workflow_inputs(&workflow, &mapped) {
            bail!(
                "composite {} stopped before {}: {error}; completed steps: {}",
                record.id,
                step.step_id,
                completed.join(", ")
            );
        }
        let report = match teach_flow::execute_composite_step(
            &workflow, &mapped, true, session, headed,
        ) {
            Ok(report) => report,
            Err(error) => bail!(
                "composite {} stopped at {}; completed steps: {}. Reconcile external state before retrying. Cause: {error:#}",
                record.id,
                step.step_id,
                completed.join(", ")
            ),
        };
        let verified = verify_audit(&workflow, &report.audit_path);
        let run_id = report.run_id.clone();
        let audit_path = report.audit_path.clone();
        let package_digest =
            agentmesh_teach::compile_workflow_capability(&workflow)?.content_digest()?;
        let step_result = json!({
            "workflow": workflow.id,
            "status": if verified { "verified" } else { "completed_unverified" },
            "outputs": report.outputs,
            "artifacts": report.artifacts,
            "receipt": {
                "run_id": report.run_id,
                "audit_path": report.audit_path,
                "capability_version": workflow.version,
                "package_digest": package_digest,
                "status": if verified { "verified" } else { "completed" },
                "policy_decisions": if teach_flow::workflow_has_write(&workflow) { vec!["cli_composite_approval_confirmed"] } else { vec!["no_write_effect_declared"] }
            }
        });
        completed.push(format!(
            "{} (run {}; audit {})",
            step.step_id,
            run_id,
            audit_path.display()
        ));
        step_results.insert(step.step_id.clone(), step_result);
        println!(
            "✓ {} — {}",
            step.step_id,
            if verified {
                "verified"
            } else {
                "completed, unverified"
            }
        );
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "id": record.id,
            "package_digest": record.package_digest,
            "status": if step_results.values().all(|result| result["status"] == "verified") { "verified" } else { "completed_unverified" },
            "steps": step_results,
            "recovery": "reconcile external state before retrying after any partial failure"
        }))?
    );
    Ok(())
}

fn resolve_value(
    value: &Value,
    inputs: &Map<String, Value>,
    prior: &Map<String, Value>,
) -> Result<Value> {
    match value {
        Value::Array(items) => items
            .iter()
            .map(|item| resolve_value(item, inputs, prior))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array),
        Value::Object(object)
            if object.len() == 1 && object.get("$input").and_then(Value::as_str).is_some() =>
        {
            let name = object["$input"].as_str().expect("checked as string");
            inputs
                .get(name)
                .cloned()
                .with_context(|| format!("missing composite input {name}"))
        }
        Value::Object(object)
            if object.len() == 2 && object.get("$step").and_then(Value::as_str).is_some() =>
        {
            let step_id = object["$step"].as_str().expect("checked as string");
            let path = object
                .get("path")
                .and_then(Value::as_str)
                .context("output reference requires path")?;
            let name = path
                .strip_prefix("outputs.")
                .context("output reference path must be outputs.NAME")?;
            prior
                .get(step_id)
                .and_then(|result| result.get("outputs"))
                .and_then(Value::as_object)
                .and_then(|outputs| outputs.get(name))
                .cloned()
                .with_context(|| format!("step {step_id} produced no output {name}"))
        }
        Value::Object(object) => object
            .iter()
            .map(|(key, item)| Ok((key.clone(), resolve_value(item, inputs, prior)?)))
            .collect::<Result<Map<_, _>>>()
            .map(Value::Object),
        _ => Ok(value.clone()),
    }
}

fn parse_inputs(raw: &[String]) -> Result<Map<String, Value>> {
    let mut inputs = Map::new();
    for item in raw {
        let (name, value) = item
            .split_once('=')
            .with_context(|| format!("input {item:?} must be KEY=VALUE"))?;
        if name.is_empty() {
            bail!("input name cannot be empty");
        }
        let value =
            serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()));
        if inputs.insert(name.to_string(), value).is_some() {
            bail!("input {name} was supplied more than once");
        }
    }
    Ok(inputs)
}

fn validate_workflow_inputs(
    workflow: &agentmesh_teach::Workflow,
    inputs: &Map<String, Value>,
) -> Result<()> {
    for (name, definition) in &workflow.inputs {
        if !inputs.contains_key(name) && definition.is_required() {
            bail!("missing required input {name}");
        }
        if let Some(value) = inputs.get(name) {
            let valid = match definition.input_type {
                agentmesh_teach::InputType::String | agentmesh_teach::InputType::Datetime => {
                    value.is_string()
                }
                agentmesh_teach::InputType::Integer => value.as_i64().is_some(),
                agentmesh_teach::InputType::Number => value.is_number(),
                agentmesh_teach::InputType::Boolean => value.is_boolean(),
                agentmesh_teach::InputType::Array => value.is_array(),
                agentmesh_teach::InputType::Object => value.is_object(),
            };
            if !valid {
                bail!(
                    "input {name} has the wrong type for workflow {}",
                    workflow.id
                );
            }
        }
    }
    for name in inputs.keys() {
        if !workflow.inputs.contains_key(name) {
            bail!("unknown input {name} for workflow {}", workflow.id);
        }
    }
    Ok(())
}

fn validate_input_schema(schema: &Value) -> Result<()> {
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        bail!("input_schema.type must be object");
    }
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .context("input_schema.properties must be an object")?;
    let required = match schema.get("required") {
        Some(Value::Array(required)) => required.clone(),
        Some(_) => bail!("input_schema.required must be an array"),
        None => Vec::new(),
    };
    for name in required.iter().filter_map(Value::as_str) {
        if !properties.contains_key(name) {
            bail!("required input {name} is not declared in properties");
        }
    }
    for (name, value) in properties {
        if !valid_step_id(name) || is_credential_name(name) {
            bail!("invalid or credential-like composite input name {name}");
        }
        validate_schema_node(value, 0)
            .with_context(|| format!("invalid schema for input {name}"))?;
    }
    Ok(())
}

fn validate_schema_node(schema: &Value, depth: usize) -> Result<()> {
    if depth > 8 {
        bail!("input schemas may nest at most eight levels");
    }
    let kind = schema
        .get("type")
        .and_then(Value::as_str)
        .context("input schema properties need a type")?;
    if let Some(values) = schema.get("enum") {
        if !values.as_array().is_some_and(|values| !values.is_empty()) {
            bail!("enum must be a non-empty array");
        }
    }
    for key in ["minimum", "maximum"] {
        if schema.get(key).is_some_and(|value| !value.is_number()) {
            bail!("{key} must be a number");
        }
    }
    if let (Some(minimum), Some(maximum)) = (schema["minimum"].as_f64(), schema["maximum"].as_f64())
    {
        if minimum > maximum {
            bail!("minimum cannot exceed maximum");
        }
    }
    for key in ["minLength", "maxLength", "minItems", "maxItems"] {
        if schema
            .get(key)
            .is_some_and(|value| value.as_u64().is_none())
        {
            bail!("{key} must be a non-negative integer");
        }
    }
    match kind {
        "string" | "integer" | "number" | "boolean" => Ok(()),
        "array" => validate_schema_node(
            schema.get("items").context("array schema needs items")?,
            depth + 1,
        ),
        "object" => {
            let properties = schema
                .get("properties")
                .and_then(Value::as_object)
                .context("object schema needs properties")?;
            let required = match schema.get("required") {
                Some(Value::Array(required)) => required.as_slice(),
                Some(_) => bail!("nested required must be an array"),
                None => &[],
            };
            for name in required.iter().filter_map(Value::as_str) {
                if !properties.contains_key(name) {
                    bail!("required input {name} is not declared");
                }
            }
            for (name, child) in properties {
                if !valid_step_id(name) {
                    bail!("invalid nested input name {name}");
                }
                validate_schema_node(child, depth + 1)?;
            }
            Ok(())
        }
        other => bail!("unsupported input schema type {other:?}"),
    }
}

fn validate_value(value: &Value, schema: &Value, path: &str) -> Result<()> {
    let kind = schema
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("object");
    let valid = match kind {
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    };
    if !valid {
        bail!("{path} must be {kind}");
    }
    if let Some(choices) = schema.get("enum").and_then(Value::as_array) {
        if !choices.contains(value) {
            bail!("{path} is not an allowed value");
        }
    }
    if let Some(text) = value.as_str() {
        if schema
            .get("minLength")
            .and_then(Value::as_u64)
            .is_some_and(|min| text.chars().count() < min as usize)
        {
            bail!("{path} is too short");
        }
        if schema
            .get("maxLength")
            .and_then(Value::as_u64)
            .is_some_and(|max| text.chars().count() > max as usize)
        {
            bail!("{path} is too long");
        }
    }
    if let Some(number) = value.as_f64() {
        if schema
            .get("minimum")
            .and_then(Value::as_f64)
            .is_some_and(|minimum| number < minimum)
        {
            bail!("{path} is below its minimum");
        }
        if schema
            .get("maximum")
            .and_then(Value::as_f64)
            .is_some_and(|maximum| number > maximum)
        {
            bail!("{path} is above its maximum");
        }
    }
    if let Some(object) = value.as_object() {
        let properties = schema
            .get("properties")
            .and_then(Value::as_object)
            .context("object schema needs properties")?;
        for name in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !object.contains_key(name) {
                bail!("missing required input {path}.{name}");
            }
        }
        for (name, child) in object {
            if let Some(child_schema) = properties.get(name) {
                validate_value(child, child_schema, &format!("{path}.{name}"))?;
            } else if schema.get("additionalProperties").and_then(Value::as_bool) != Some(true) {
                bail!("unknown input {path}.{name}");
            }
        }
    }
    if let Some(items) = value.as_array() {
        if schema
            .get("minItems")
            .and_then(Value::as_u64)
            .is_some_and(|min| items.len() < min as usize)
        {
            bail!("{path} has too few items");
        }
        if schema
            .get("maxItems")
            .and_then(Value::as_u64)
            .is_some_and(|max| items.len() > max as usize)
        {
            bail!("{path} has too many items");
        }
        let item_schema = schema.get("items").context("array schema needs items")?;
        for (index, item) in items.iter().enumerate() {
            validate_value(item, item_schema, &format!("{path}[{index}]"))?;
        }
    }
    Ok(())
}

fn apply_schema_defaults(value: &mut Value, schema: &Value) {
    let (Some(object), Some(properties)) = (
        value.as_object_mut(),
        schema.get("properties").and_then(Value::as_object),
    ) else {
        return;
    };
    for (name, child_schema) in properties {
        if !object.contains_key(name) {
            if let Some(default) = child_schema.get("default") {
                object.insert(name.clone(), default.clone());
            }
        }
        if let Some(child) = object.get_mut(name) {
            apply_schema_defaults(child, child_schema);
        }
    }
}

fn verify_audit(workflow: &agentmesh_teach::Workflow, audit_path: &Path) -> bool {
    if workflow.success.is_empty() {
        return false;
    }
    let Ok(contents) = std::fs::read_to_string(audit_path) else {
        return false;
    };
    let checked = contents
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| {
            event.get("event").and_then(Value::as_str) == Some("assertion.checked")
                && event.get("phase").and_then(Value::as_str) == Some("success")
                && event.get("matched").and_then(Value::as_bool) == Some(true)
        })
        .count();
    checked == workflow.success.len()
}

fn effect_name(effect: EffectKind) -> &'static str {
    match effect {
        EffectKind::ReadOnly => "read_only",
        EffectKind::DataWrite => "data_write",
        EffectKind::ExternalCommunication => "external_communication",
        EffectKind::Financial => "financial",
        EffectKind::Destructive => "destructive",
        EffectKind::Irreversible => "irreversible",
        EffectKind::CredentialAccess => "credential_access",
        EffectKind::NetworkEgress => "network_egress",
    }
}

fn risk_for(effects: &BTreeSet<EffectKind>) -> String {
    if effects
        .iter()
        .any(|effect| matches!(effect, EffectKind::Destructive | EffectKind::Irreversible))
    {
        "critical".into()
    } else if effects.iter().any(|effect| {
        matches!(
            effect,
            EffectKind::Financial | EffectKind::CredentialAccess | EffectKind::NetworkEgress
        )
    }) {
        "high".into()
    } else if effects.iter().any(|effect| {
        matches!(
            effect,
            EffectKind::DataWrite | EffectKind::ExternalCommunication
        )
    }) {
        "medium".into()
    } else {
        "low".into()
    }
}

fn qualify_id(id: &str) -> String {
    if id.contains('.') {
        id.to_string()
    } else {
        format!("composite.{id}")
    }
}

fn valid_namespaced_id(id: &str) -> bool {
    id.split('.').count() >= 2 && id.split('.').all(valid_capability_segment)
}

fn valid_capability_segment(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.as_bytes()[0].is_ascii_lowercase()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_step_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.as_bytes()[0].is_ascii_lowercase()
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

fn is_credential_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "password",
        "secret",
        "token",
        "credential",
        "auth",
        "api_key",
        "api-key",
    ]
    .iter()
    .any(|needle| name.contains(needle))
}
