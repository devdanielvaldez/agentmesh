//! Deterministic trace-to-IR inference.
//!
//! No models, no heuristics-that-guess: the rules below turn a semantic
//! trace into draft steps plus parameter candidates, and every candidate is
//! confirmed interactively before it becomes an input. Multi-demonstration
//! learning generalizes by comparison — values that differ across demos of
//! the same capability become parameters.

use crate::{
    ApiObservation, InputDef, InputType, RecordedTarget, SemanticEvent, Step, Target, TraceKind,
    TraceValue,
};

/// A literal value observed during recording, awaiting a parameter name.
#[derive(Debug, Clone)]
pub struct ParamCandidate {
    /// Observed literal text.
    pub value: String,
    /// Suggested parameter name derived from the target.
    pub suggested_name: String,
    /// Index of the draft step using the value.
    pub step_index: usize,
}

/// Draft steps plus candidates and notes from one trace.
#[derive(Debug, Clone, Default)]
pub struct InferredDraft {
    /// Steps in recording order, deduplicated.
    pub steps: Vec<Step>,
    /// Literal values the user may promote to inputs.
    pub candidates: Vec<ParamCandidate>,
    /// Human-readable notes (skipped events, network observations, ...).
    pub warnings: Vec<String>,
    /// Sanitized request shapes observed during the demonstration.
    pub observed_apis: Vec<ApiObservation>,
}

/// Infers draft steps and parameter candidates from a trace.
///
/// Out-of-scope events are dropped (the recorder marks them); network calls
/// are counted but never become steps — the UI implementation is the
/// default, network execution arrives with a later milestone.
///
/// # Panics
///
/// Panics when a select event carries a chosen option but no target. The
/// recorder always pairs the two, so that combination means corrupt input.
#[must_use]
pub fn infer_draft(events: &[SemanticEvent]) -> InferredDraft {
    infer(events, false)
}

/// Infers one step per recorded event: repeats of the same field are kept as
/// separate literal steps and no parameter candidates are produced. Opt-in
/// via `teach --raw` for demonstrations that must replay verbatim.
///
/// # Panics
///
/// Same as [`infer_draft`].
#[must_use]
pub fn infer_draft_raw(events: &[SemanticEvent]) -> InferredDraft {
    infer(events, true)
}

fn infer(events: &[SemanticEvent], raw: bool) -> InferredDraft {
    let mut draft = InferredDraft::default();
    let mut network_calls = 0_u64;
    let mut counter = 0_u32;
    for event in events {
        if event.out_of_scope {
            continue;
        }
        match event.kind {
            TraceKind::SessionStart | TraceKind::SessionStop | TraceKind::NetworkResponse => (),
            TraceKind::NetworkCall => {
                network_calls += 1;
                if let (Some(method), Some(host), Some(path)) =
                    (&event.method, &event.host, &event.path)
                {
                    let observation = ApiObservation {
                        method: method.to_ascii_uppercase(),
                        host: host.clone(),
                        path: path.clone(),
                        query_keys: event.query_keys.clone(),
                    };
                    if !draft.observed_apis.contains(&observation) {
                        draft.observed_apis.push(observation);
                    }
                }
            }
            TraceKind::Navigate => infer_navigate(&mut draft, event, &mut counter),
            TraceKind::Click => infer_click(&mut draft, event, &mut counter),
            // Form submissions are almost always an Enter keypress on a
            // field (login forms especially): replaying them as a click on
            // the field would do nothing, so they become an explicit key
            // press the executor replays with keyboard semantics.
            TraceKind::Submit => infer_press(&mut draft, event, &mut counter),
            TraceKind::Fill => infer_fill(&mut draft, event, &mut counter, raw),
            TraceKind::Select => infer_select(&mut draft, event, &mut counter, raw),
            TraceKind::Extract => infer_extract(&mut draft, event, &mut counter),
        }
    }
    if network_calls > 0 {
        draft.warnings.push(format!(
            "{network_calls} network call(s) observed; the draft keeps the UI implementation"
        ));
    }
    draft
}

fn infer_navigate(draft: &mut InferredDraft, event: &SemanticEvent, counter: &mut u32) {
    if let Some(url) = event.url.clone().filter(|url| !url.is_empty()) {
        // Only the entry navigation opens a page. Mid-flow navigations are
        // effects of earlier interactions (a submit redirecting, an SPA
        // route change): replaying them as fresh loads would discard page
        // state, so they become URL expectations instead.
        let replayed = draft.steps.iter().any(|step| step.op == "browser.navigate");
        let (id, op) = if replayed {
            (step_id("expect", counter), "assert.url".to_string())
        } else {
            (step_id("open", counter), "browser.navigate".to_string())
        };
        push_step(
            draft,
            Step {
                id,
                op,
                target: None,
                value: None,
                url: Some(url),
                limit: None,
                timeout_ms: None,
                condition: None,
                iterations: None,
                destination: None,
                path: None,
            },
        );
    }
}

fn infer_click(draft: &mut InferredDraft, event: &SemanticEvent, counter: &mut u32) {
    if let Some(target) = event.target.as_ref().map(describe) {
        push_step(
            draft,
            Step {
                id: step_id("click", counter),
                op: "ui.click".to_string(),
                target: Some(target),
                value: None,
                url: None,
                limit: None,
                timeout_ms: None,
                condition: None,
                iterations: None,
                destination: None,
                path: None,
            },
        );
    }
}

fn infer_press(draft: &mut InferredDraft, event: &SemanticEvent, counter: &mut u32) {
    if let Some(target) = event.target.as_ref().map(describe) {
        push_step(
            draft,
            Step {
                id: step_id("press", counter),
                op: "ui.press".to_string(),
                target: Some(target),
                value: Some("Enter".to_string()),
                url: None,
                limit: None,
                timeout_ms: None,
                condition: None,
                iterations: None,
                destination: None,
                path: None,
            },
        );
    }
}

fn infer_fill(draft: &mut InferredDraft, event: &SemanticEvent, counter: &mut u32, raw: bool) {
    let Some(recorded) = &event.target else {
        return;
    };
    let target = describe(recorded);
    let key = target_key(&target);
    if raw {
        let value = match &event.value {
            Some(TraceValue::Literal { literal }) => Some(literal.clone()),
            Some(TraceValue::SecretRef { secret_ref }) => Some(secret_ref.clone()),
            None => None,
        };
        if let Some(value) = value {
            push_step(
                draft,
                Step {
                    id: step_id("fill", counter),
                    op: "ui.fill".to_string(),
                    target: Some(target),
                    value: Some(value),
                    url: None,
                    limit: None,
                    timeout_ms: None,
                    condition: None,
                    iterations: None,
                    destination: None,
                    path: None,
                },
            );
        }
        return;
    }
    match &event.value {
        Some(TraceValue::SecretRef { secret_ref }) => {
            push_step(
                draft,
                Step {
                    id: step_id("fill", counter),
                    op: "ui.fill".to_string(),
                    target: Some(target),
                    value: Some(secret_ref.clone()),
                    url: None,
                    limit: None,
                    timeout_ms: None,
                    condition: None,
                    iterations: None,
                    destination: None,
                    path: None,
                },
            );
        }
        Some(TraceValue::Literal { literal }) => {
            let repeat = draft
                .steps
                .iter()
                .rposition(|step| step.op == "ui.fill" && step_target_key(step) == key);
            if let Some(index) = repeat {
                // Same field filled again: keep the last value
                // and refresh its candidate in place.
                let suggested_name = suggest_name(recorded, draft.candidates.len());
                let placeholder = format!("{{{{ inputs.{suggested_name} }}}}");
                if let Some(candidate) = draft
                    .candidates
                    .iter_mut()
                    .find(|candidate| candidate.step_index == index)
                {
                    candidate.value.clone_from(literal);
                    candidate.suggested_name = suggested_name;
                } else {
                    draft.candidates.push(ParamCandidate {
                        value: literal.clone(),
                        suggested_name,
                        step_index: index,
                    });
                }
                draft.steps[index].value = Some(placeholder);
            } else {
                let value = candidate_placeholder(draft, literal, recorded);
                push_step(
                    draft,
                    Step {
                        id: step_id("fill", counter),
                        op: "ui.fill".to_string(),
                        target: Some(target),
                        value: Some(value),
                        url: None,
                        limit: None,
                        timeout_ms: None,
                        condition: None,
                        iterations: None,
                        destination: None,
                        path: None,
                    },
                );
            }
        }
        None => (),
    }
}

fn infer_extract(draft: &mut InferredDraft, event: &SemanticEvent, counter: &mut u32) {
    if let Some(target) = event.target.as_ref().map(describe) {
        push_step(
            draft,
            Step {
                id: step_id("read", counter),
                op: "ui.extract".to_string(),
                target: Some(target),
                value: None,
                url: None,
                limit: None,
                timeout_ms: None,
                condition: None,
                iterations: None,
                destination: None,
                path: None,
            },
        );
    } else {
        draft
            .warnings
            .push("recorded an extraction probe without a target; skipped".to_string());
    }
}

fn infer_select(draft: &mut InferredDraft, event: &SemanticEvent, counter: &mut u32, raw: bool) {
    if let Some(target) = event.target.as_ref().map(describe) {
        let selected = event.selected.clone().unwrap_or_default();
        let value = if selected.is_empty() {
            None
        } else if raw {
            Some(selected)
        } else {
            Some(candidate_placeholder(
                draft,
                &selected,
                event.target.as_ref().expect("selected has a target"),
            ))
        };
        push_step(
            draft,
            Step {
                id: step_id("select", counter),
                op: "ui.select".to_string(),
                target: Some(target),
                value,
                url: None,
                limit: None,
                timeout_ms: None,
                condition: None,
                iterations: None,
                destination: None,
                path: None,
            },
        );
    }
}

/// Finds literals that differ between a stored workflow and a fresh draft:
/// each divergence is a value the new demonstration suggests parameterizing.
/// Steps align by `(op, target-key)`; the fresh side compares the observed
/// candidate value (drafts parameterize literals into placeholders).
#[must_use]
pub fn find_divergent_literals(
    workflow: &crate::Workflow,
    draft: &InferredDraft,
) -> Vec<Divergence> {
    let mut divergences = Vec::new();
    let observed: std::collections::BTreeMap<usize, &str> = draft
        .candidates
        .iter()
        .map(|candidate| (candidate.step_index, candidate.value.as_str()))
        .collect();
    let mut draft_by_key: std::collections::BTreeMap<(String, String), usize> =
        std::collections::BTreeMap::new();
    for (index, step) in draft.steps.iter().enumerate() {
        draft_by_key.insert((step.op.clone(), step_target_key(step)), index);
    }
    for step in &workflow.steps {
        let key = (step.op.clone(), step_target_key(step));
        let (Some(old), Some(&fresh_index)) =
            (literal_of(step.value.as_ref()), draft_by_key.get(&key))
        else {
            continue;
        };
        if let Some(&new) = observed.get(&fresh_index) {
            if old != new {
                divergences.push(Divergence {
                    step_id: step.id.clone(),
                    old: old.to_string(),
                    new: new.to_string(),
                });
            }
        }
    }
    divergences
}

/// One value that changed between demonstrations.
#[derive(Debug, Clone)]
pub struct Divergence {
    /// Stored step id.
    pub step_id: String,
    /// Value in the stored workflow.
    pub old: String,
    /// Value in the fresh demonstration.
    pub new: String,
}

/// Pushes a step onto the draft.
fn push_step(draft: &mut InferredDraft, step: Step) {
    draft.steps.push(step);
}

/// Generates `fill_1`, `click_2`, ... ids unique within the draft.
fn step_id(prefix: &str, counter: &mut u32) -> String {
    *counter += 1;
    format!("{prefix}_{counter}")
}

/// Converts a recorded descriptor into an IR target. Visible text and
/// placeholders survive (truncated) as resolution fallbacks for elements
/// without names.
fn describe(recorded: &RecordedTarget) -> Target {
    let clipped = |value: &str| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.chars().take(100).collect())
        }
    };
    Target {
        semantic: None,
        role: optional(recorded.role.clone()),
        accessible_name: optional(recorded.name.clone()),
        text: clipped(&recorded.text),
        placeholder: clipped(&recorded.placeholder),
        autocomplete: optional(recorded.autocomplete.trim().to_string()),
        selectors: recorded
            .selectors
            .iter()
            .filter(|selector| !selector.is_empty())
            .take(2)
            .cloned()
            .collect(),
        match_pattern: None,
    }
}

/// Registers a literal as a parameter candidate and returns its placeholder.
fn candidate_placeholder(
    draft: &mut InferredDraft,
    literal: &str,
    recorded: &RecordedTarget,
) -> String {
    let suggested_name = suggest_name(recorded, draft.candidates.len());
    let placeholder = format!("{{{{ inputs.{suggested_name} }}}}");
    draft.candidates.push(ParamCandidate {
        value: literal.to_string(),
        suggested_name,
        step_index: draft.steps.len(),
    });
    placeholder
}

/// Suggests a parameter name from the target, falling back to `value_N`.
/// Framework-generated ids (React `useId` like `_R_abc123`, `Ember` `ember42`,
/// `auto-component-<uuid>`) rotate on every page load, so they never become
/// names — accepting one as a parameter bakes a stale id into the workflow's
/// input contract.
fn suggest_name(recorded: &RecordedTarget, index: usize) -> String {
    let mut candidates = vec![recorded.name.clone(), recorded.placeholder.clone()];
    if !is_generated_id(&recorded.autocomplete) {
        candidates.push(recorded.autocomplete.clone());
    }
    if !is_generated_id(&recorded.id) {
        candidates.push(recorded.id.clone());
    }
    for candidate in candidates {
        let slug: String = candidate
            .to_lowercase()
            .chars()
            .map(|char| {
                if char.is_ascii_alphanumeric() {
                    char
                } else {
                    '_'
                }
            })
            .collect::<String>()
            .split('_')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("_");
        if !slug.is_empty() {
            return slug;
        }
    }
    format!("value_{}", index + 1)
}

/// True for ids a framework mints per render: useless as stable names.
fn is_generated_id(id: &str) -> bool {
    id.starts_with('_') || id.starts_with("ember") || id.contains("auto-component")
}

/// Stable key identifying the element a step acts on.
fn step_target_key(step: &Step) -> String {
    step.target.as_ref().map_or_else(String::new, target_key)
}

/// Stable key for a target descriptor: role plus accessible name. Selectors
/// are healing fallbacks, not identity, so they stay out of the key.
fn target_key(target: &Target) -> String {
    format!(
        "{}|{}",
        target.role.as_deref().unwrap_or(""),
        target.accessible_name.as_deref().unwrap_or("")
    )
}

/// Returns the literal text of a template-free value.
fn literal_of(value: Option<&String>) -> Option<&str> {
    value
        .map(String::as_str)
        .filter(|text| !text.contains("{{") && !text.starts_with("secret://"))
}

/// Empties become None.
fn optional(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

/// Finds every `secret://` reference used by a workflow, ordered and
/// deduplicated. The secret broker resolves these at runtime; they never
/// carry values.
#[must_use]
pub fn find_secret_refs(workflow: &crate::Workflow) -> Vec<String> {
    let mut refs = std::collections::BTreeSet::new();
    for step in &workflow.steps {
        for text in [
            &step.value,
            &step.limit,
            &step.url,
            &step
                .target
                .as_ref()
                .and_then(|target| target.match_pattern.clone()),
        ]
        .into_iter()
        .flatten()
        {
            refs.extend(scan_secret_refs(text));
        }
    }
    refs.into_iter().collect()
}

/// Every `secret://` reference used by a draft's steps, ordered and
/// deduplicated. Used at review time so the user can rename each secret
/// before the workflow is saved.
#[must_use]
pub fn draft_secret_refs(draft: &InferredDraft) -> Vec<String> {
    let mut refs = std::collections::BTreeSet::new();
    for step in &draft.steps {
        for text in [&step.value, &step.limit, &step.url].into_iter().flatten() {
            refs.extend(scan_secret_refs(text));
        }
    }
    refs.into_iter().collect()
}

/// Normalizes a user-supplied secret name into the slug stored after
/// `secret://` (`"LinkedIn Password"` → `"linkedin_password"`). Leading
/// `secret://` and `SECRET_` prefixes are stripped so env-var style answers
/// work too.
#[must_use]
pub fn normalize_secret_slug(name: &str) -> String {
    let trimmed = name.trim();
    let trimmed = trimmed
        .strip_prefix("secret://")
        .or_else(|| trimmed.strip_prefix("SECRET_"))
        .or_else(|| trimmed.strip_prefix("secret_"))
        .unwrap_or(trimmed);
    let mut out = String::new();
    let mut last_underscore = true;
    for char in trimmed.chars() {
        if char.is_ascii_alphanumeric() {
            out.push(char.to_ascii_lowercase());
            last_underscore = false;
        } else if !last_underscore {
            out.push('_');
            last_underscore = true;
        }
    }
    while out.starts_with('_') {
        out.remove(0);
    }
    while out.ends_with('_') {
        out.pop();
    }
    out
}

/// Suggests a stable secret slug for a recorded reference. Auto-generated
/// refs embed unstable page ids (`secret://host/_R_abc123`), so the
/// suggestion keeps the host core plus a password hint instead.
#[must_use]
pub fn suggest_secret_slug(reference: &str) -> String {
    let body = reference.strip_prefix("secret://").unwrap_or(reference);
    let mut parts = body.split('/');
    let host = parts.next().unwrap_or("");
    let core = host
        .strip_prefix("www.")
        .unwrap_or(host)
        .split('.')
        .next()
        .unwrap_or(host);
    let slug = normalize_secret_slug(&format!("{core}_password"));
    if slug.is_empty() {
        "password".to_string()
    } else {
        slug
    }
}

/// Extracts `secret://` tokens (terminated by whitespace, quote, or brace).
fn scan_secret_refs(text: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("secret://") {
        let tail = &rest[start + "secret://".len()..];
        let end = tail
            .find(|char: char| char.is_whitespace() || matches!(char, '"' | '\'' | '}' | ',' | ')'))
            .unwrap_or(tail.len());
        if end > 0 {
            refs.push(format!("secret://{}", &tail[..end]));
        }
        rest = &tail[end.min(tail.len())..];
    }
    refs
}

/// Maps a `secret://` reference to the environment variable the runtime
/// reads (`secret://app/password` → `SECRET_APP_PASSWORD`). Mirrors the
/// executor mapping exactly; agents use secrets without ever seeing them.
#[must_use]
pub fn secret_env_var(reference: &str) -> String {
    let mut out = String::from("SECRET_");
    let mut last_underscore = true;
    for char in reference
        .strip_prefix("secret://")
        .unwrap_or(reference)
        .chars()
    {
        if char.is_ascii_alphanumeric() {
            out.push(char.to_ascii_uppercase());
            last_underscore = false;
        } else if !last_underscore {
            out.push('_');
            last_underscore = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    out
}

/// Builds an [`InputDef`] for a confirmed literal candidate, guessing the
/// type from the observed text (integers, booleans, otherwise strings).
#[must_use]
pub fn candidate_input(value: &str) -> InputDef {
    let input_type = if value.parse::<i64>().is_ok() {
        InputType::Integer
    } else if matches!(value.to_lowercase().as_str(), "true" | "false") {
        InputType::Boolean
    } else {
        InputType::String
    };
    InputDef {
        input_type,
        required: Some(true),
        default: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TraceKind;

    fn fill_event(name: &str, literal: &str) -> SemanticEvent {
        SemanticEvent {
            seq: 0,
            ts_ms: 0,
            kind: TraceKind::Fill,
            url: Some("https://app.example/".to_string()),
            out_of_scope: false,
            target: Some(RecordedTarget {
                name: name.to_string(),
                selectors: vec!["#contact".to_string()],
                ..RecordedTarget::default()
            }),
            value: Some(TraceValue::Literal {
                literal: literal.to_string(),
            }),
            selected: None,
            method: None,
            host: None,
            path: None,
            query_keys: Vec::new(),
            status: None,
        }
    }

    #[test]
    fn network_learning_keeps_shapes_but_not_query_values() {
        let event = SemanticEvent {
            kind: TraceKind::NetworkCall,
            method: Some("post".to_string()),
            host: Some("api.example.com".to_string()),
            path: Some("/v1/search".to_string()),
            query_keys: vec!["q".to_string(), "page".to_string()],
            ..fill_event("ignored", "secret-query-value")
        };
        let draft = infer_draft(&[event]);
        assert_eq!(draft.observed_apis.len(), 1);
        assert_eq!(draft.observed_apis[0].method, "POST");
        let encoded = serde_json::to_string(&draft.observed_apis).expect("serialize API hints");
        assert!(!encoded.contains("secret-query-value"));
    }

    #[test]
    fn fills_become_candidates_and_dedupe() {
        let events = vec![
            SemanticEvent {
                kind: TraceKind::Navigate,
                url: Some("https://app.example/".to_string()),
                ..fill_event("contact", "Maria")
            },
            fill_event("contact", "Daniel"),
            fill_event("contact", "Maria"),
        ];
        let draft = infer_draft(&events);
        assert_eq!(draft.steps.len(), 2);
        assert_eq!(draft.candidates.len(), 1);
        assert_eq!(draft.candidates[0].value, "Maria");
        assert_eq!(draft.candidates[0].suggested_name, "contact");
        assert!(
            draft.steps[1]
                .value
                .as_deref()
                .unwrap()
                .contains("inputs.contact")
        );
    }

    #[test]
    fn raw_mode_keeps_every_keystroke_as_its_own_literal_step() {
        let events = vec![fill_event("contact", "P"), fill_event("contact", "Pr")];
        let draft = infer_draft_raw(&events);
        assert_eq!(draft.steps.len(), 2);
        assert!(draft.candidates.is_empty());
        let values: Vec<&str> = draft
            .steps
            .iter()
            .map(|step| step.value.as_deref().unwrap())
            .collect();
        assert_eq!(values, vec!["P", "Pr"]);
    }

    #[test]
    fn repeat_navigations_become_url_expectations() {
        let navigate = |url: &str| SemanticEvent {
            kind: TraceKind::Navigate,
            url: Some(url.to_string()),
            ..fill_event("x", "")
        };
        let draft = infer_draft(&[
            navigate("https://app.example/"),
            navigate("https://app.example/"),
            navigate("https://app.example/quotes"),
        ]);
        assert_eq!(draft.steps.len(), 3);
        assert_eq!(draft.steps[0].op, "browser.navigate");
        assert_eq!(draft.steps[0].id, "open_1");
        assert_eq!(draft.steps[1].op, "assert.url");
        assert_eq!(draft.steps[1].id, "expect_2");
        assert_eq!(draft.steps[2].op, "assert.url");
        assert_eq!(
            draft.steps[2].url.as_deref(),
            Some("https://app.example/quotes")
        );
    }

    #[test]
    fn visible_text_survives_truncated_as_a_fallback_signal() {
        let mut event = fill_event("go", "");
        event.kind = TraceKind::Click;
        event.target.as_mut().expect("fill_event has a target").text =
            format!("Go please {}", "x".repeat(200));
        let draft = infer_draft(&[event]);
        assert_eq!(draft.steps.len(), 1);
        let text = draft.steps[0]
            .target
            .as_ref()
            .expect("click keeps a target")
            .text
            .clone();
        assert!(text.is_some());
        assert!(text.unwrap().len() <= 100);
    }

    #[test]
    fn placeholder_survives_as_a_fill_signal() {
        let mut event = fill_event("phone", "809");
        event
            .target
            .as_mut()
            .expect("fill_event has a target")
            .placeholder = "Ej. 849-342-1998".to_string();
        let draft = infer_draft(&[event]);
        assert_eq!(draft.steps.len(), 1);
        assert_eq!(
            draft.steps[0]
                .target
                .as_ref()
                .expect("fill keeps a target")
                .placeholder
                .as_deref(),
            Some("Ej. 849-342-1998")
        );
    }

    #[test]
    fn extract_probes_become_read_steps() {
        let event = SemanticEvent {
            kind: TraceKind::Extract,
            ..fill_event("plan", "")
        };
        let draft = infer_draft(&[event]);
        assert_eq!(draft.steps.len(), 1);
        assert_eq!(draft.steps[0].op, "ui.extract");
        assert!(draft.steps[0].id.starts_with("read_"));
        assert!(draft.candidates.is_empty());
    }

    #[test]
    fn raw_mode_keeps_select_values_literal() {
        let event = SemanticEvent {
            kind: TraceKind::Select,
            selected: Some("Turbo SIM - RD$ 2000".to_string()),
            ..fill_event("plan", "")
        };
        let draft = infer_draft_raw(&[event]);
        assert_eq!(draft.steps.len(), 1);
        assert!(draft.candidates.is_empty());
        assert_eq!(
            draft.steps[0].value.as_deref(),
            Some("Turbo SIM - RD$ 2000")
        );
    }

    #[test]
    fn secrets_pass_through_untouched() {
        let mut event = fill_event("pw", "s3cret");
        event.value = Some(TraceValue::SecretRef {
            secret_ref: "secret://app/pw".to_string(),
        });
        let draft = infer_draft(&[event]);
        assert_eq!(draft.steps.len(), 1);
        assert_eq!(draft.steps[0].value.as_deref(), Some("secret://app/pw"));
        assert!(draft.candidates.is_empty());
    }

    #[test]
    fn submits_become_enter_presses() {
        let event = SemanticEvent {
            kind: TraceKind::Submit,
            ..fill_event("pw", "")
        };
        let draft = infer_draft(&[event]);
        assert_eq!(draft.steps.len(), 1);
        assert_eq!(draft.steps[0].op, "ui.press");
        assert!(draft.steps[0].id.starts_with("press_"));
        assert_eq!(draft.steps[0].value.as_deref(), Some("Enter"));
    }

    #[test]
    fn secret_slugs_normalize_user_names() {
        assert_eq!(
            normalize_secret_slug("LinkedIn Password"),
            "linkedin_password"
        );
        assert_eq!(
            normalize_secret_slug("SECRET_LINKEDIN_PASSWORD"),
            "linkedin_password"
        );
        assert_eq!(
            normalize_secret_slug("secret://linkedin/password"),
            "linkedin_password"
        );
        assert_eq!(
            suggest_secret_slug("secret://www.linkedin.com/_R_abc123"),
            "linkedin_password"
        );
        assert_eq!(
            draft_secret_refs(&InferredDraft {
                steps: vec![
                    crate::Step {
                        id: "fill_1".to_string(),
                        op: "ui.fill".to_string(),
                        target: None,
                        value: Some("secret://a/one secret://b/two".to_string()),
                        url: None,
                        limit: None,
                        timeout_ms: None,
                        condition: None,
                        iterations: None,
                        destination: None,
                        path: None,
                    },
                    crate::Step {
                        id: "fill_2".to_string(),
                        op: "ui.fill".to_string(),
                        target: None,
                        value: Some("secret://a/one".to_string()),
                        url: None,
                        limit: None,
                        timeout_ms: None,
                        condition: None,
                        iterations: None,
                        destination: None,
                        path: None,
                    },
                ],
                ..InferredDraft::default()
            }),
            vec!["secret://a/one".to_string(), "secret://b/two".to_string()]
        );
    }

    #[test]
    fn generated_ids_never_become_parameter_names() {
        let recorded = RecordedTarget {
            id: "_R_svvtiejj35659j6_".to_string(),
            autocomplete: "username".to_string(),
            ..RecordedTarget::default()
        };
        assert_eq!(suggest_name(&recorded, 0), "username");
        let recorded = RecordedTarget {
            id: "_R_svvtiejj35659j6_".to_string(),
            ..RecordedTarget::default()
        };
        assert_eq!(suggest_name(&recorded, 0), "value_1");
    }

    #[test]
    fn autocomplete_survives_as_a_resolution_signal() {
        let mut event = fill_event("pw", "s3cret");
        event.target.as_mut().expect("target").autocomplete = "current-password".to_string();
        let draft = infer_draft(&[event]);
        assert_eq!(draft.steps.len(), 1);
        assert_eq!(
            draft.steps[0]
                .target
                .as_ref()
                .and_then(|target| target.autocomplete.clone()),
            Some("current-password".to_string())
        );
    }

    #[test]
    fn secret_refs_found_and_deduped() {
        let yaml = "version: \"1.0\"\nid: app.login\nruntime: browser\nsteps:\n  - id: fill_1\n    op: ui.fill\n    target:\n      role: textbox\n    value: 'secret://app/password'\n  - id: fill_2\n    op: ui.fill\n    target:\n      role: textbox\n    value: 'user secret://app/password again'\n";
        let workflow = crate::parse_workflow(yaml).expect("stored parses");
        assert_eq!(find_secret_refs(&workflow), vec!["secret://app/password"]);
        let yaml = "version: \"1.0\"\nid: app.plain\nruntime: browser\nsteps:\n  - id: a\n    op: ui.wait\n";
        let workflow = crate::parse_workflow(yaml).expect("stored parses");
        assert!(find_secret_refs(&workflow).is_empty());
    }

    #[test]
    fn secret_env_vars_mirror_the_executor() {
        assert_eq!(
            secret_env_var("secret://app/password"),
            "SECRET_APP_PASSWORD"
        );
        assert_eq!(
            secret_env_var("secret://wa-business/api.key"),
            "SECRET_WA_BUSINESS_API_KEY"
        );
    }

    #[test]
    fn out_of_scope_events_dropped() {
        let mut event = fill_event("contact", "Daniel");
        event.out_of_scope = true;
        let draft = infer_draft(&[event]);
        assert!(draft.steps.is_empty());
    }

    #[test]
    fn divergences_detected_across_demos() {
        // Stored workflow with a literal the new demo changes.
        let yaml = "version: \"1.0\"\nid: app.search\nruntime: browser\nsteps:\n  - id: fill_1\n    op: ui.fill\n    target:\n      role: textbox\n      accessible_name: contact\n    value: 'Daniel'\n";
        let workflow = crate::parse_workflow(yaml).expect("stored parses");
        let mut demo = fill_event("contact", "Pedro");
        demo.target.as_mut().expect("target").role = "textbox".to_string();
        let draft = infer_draft(&[demo]);
        let divergences = find_divergent_literals(&workflow, &draft);
        assert_eq!(divergences.len(), 1);
        assert_eq!(divergences[0].new, "Pedro");
    }
}
