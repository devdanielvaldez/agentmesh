//! Local, deterministic capability memory.
//!
//! This module does not alter model weights. It extracts reusable semantic
//! routines from stored workflows and ranks workflows for retrieval. Literal
//! values, selectors, URLs, and secrets are deliberately excluded from the
//! routine signature.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{Step, Workflow};

/// One place where a learned routine occurs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineOccurrence {
    /// Workflow containing the routine.
    pub workflow: String,
    /// Inclusive first step index.
    pub first_step: usize,
    /// Inclusive last step index.
    pub last_step: usize,
}

/// A repeated, value-free sequence that can guide capability composition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnedRoutine {
    /// Semantic step signatures in execution order.
    pub signature: Vec<String>,
    /// Locations where the sequence was observed.
    pub occurrences: Vec<RoutineOccurrence>,
}

/// A workflow retrieval result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowMatch {
    /// Matching workflow id.
    pub workflow: String,
    /// Simple deterministic lexical relevance score.
    pub score: usize,
}

/// Mines repeated contiguous routines between `min_steps` and `max_steps`.
///
/// A sequence must occur at least twice. The signature only contains the
/// operation, semantic concept, and accessibility role, never demonstrated
/// text, selectors, URLs, or input values.
#[must_use]
pub fn mine_routines(
    workflows: &[Workflow],
    min_steps: usize,
    max_steps: usize,
) -> Vec<LearnedRoutine> {
    if min_steps == 0 || min_steps > max_steps {
        return Vec::new();
    }
    let mut found: BTreeMap<Vec<String>, Vec<RoutineOccurrence>> = BTreeMap::new();
    for workflow in workflows {
        let signatures: Vec<String> = workflow.steps.iter().map(step_signature).collect();
        for length in min_steps..=max_steps.min(signatures.len()) {
            for first_step in 0..=signatures.len() - length {
                found
                    .entry(signatures[first_step..first_step + length].to_vec())
                    .or_default()
                    .push(RoutineOccurrence {
                        workflow: workflow.id.clone(),
                        first_step,
                        last_step: first_step + length - 1,
                    });
            }
        }
    }
    let mut routines: Vec<_> = found
        .into_iter()
        .filter_map(|(signature, occurrences)| {
            (occurrences.len() >= 2).then_some(LearnedRoutine {
                signature,
                occurrences,
            })
        })
        .collect();
    routines.sort_by(|left, right| {
        right
            .signature
            .len()
            .cmp(&left.signature.len())
            .then_with(|| right.occurrences.len().cmp(&left.occurrences.len()))
            .then_with(|| left.signature.cmp(&right.signature))
    });
    routines
}

/// Operation synonyms for local retrieval: a query saying "type" should still
/// find `ui.fill` workflows, and "open" should find `browser.navigate`.
fn expand_term(term: &str) -> Vec<String> {
    let mut variants = vec![term.to_string()];
    let family: &[&str] = match term {
        "type" | "fill" | "enter" | "write" => &["fill", "type", "enter"],
        "click" | "press" | "tap" | "activate" | "submit" => {
            &["click", "press", "activate", "tap", "submit"]
        }
        "open" | "navigate" | "goto" | "visit" => &["open", "navigate", "visit"],
        "read" | "extract" | "scrape" | "list" | "lookup" | "search" => {
            &["read", "extract", "lookup", "search", "list"]
        }
        "login" | "signin" | "auth" => &["login", "auth", "session"],
        _ => &[],
    };
    for variant in family {
        if *variant != term {
            variants.push((*variant).to_string());
        }
    }
    variants
}

/// Ranks workflows against a natural-language query without a remote model.
///
/// Scoring is IDF-weighted: rare terms (a workflow id token) count more than
/// ubiquitous ones (`ui`, `browser`). Operation synonyms expand the query so
/// "type the password" still finds `ui.fill` capabilities.
#[must_use]
pub fn rank_workflows(query: &str, workflows: &[Workflow], limit: usize) -> Vec<WorkflowMatch> {
    rank_workflows_weighted(query, workflows, limit, &[])
}

/// Experience-weighted ranking: workflows with a lower observed success rate
/// are demoted so retrieval prefers capabilities that actually work.
/// `success` maps workflow id → (runs, succeeded).
#[must_use]
pub fn rank_workflows_weighted(
    query: &str,
    workflows: &[Workflow],
    limit: usize,
    success: &[(String, u64, u64)],
) -> Vec<WorkflowMatch> {
    let terms = tokens(query);
    if terms.is_empty() || limit == 0 {
        return Vec::new();
    }
    let documents: Vec<BTreeSet<String>> = workflows
        .iter()
        .map(|workflow| {
            let mut document = format!("{} {} ", workflow.id, workflow.description);
            for name in workflow.inputs.keys() {
                document.push_str(name);
                document.push(' ');
            }
            for step in &workflow.steps {
                document.push_str(&step_signature(step));
                document.push(' ');
            }
            tokens(&document)
        })
        .collect();
    let total = documents.len();
    let idf = |term: &str| {
        let containing = documents
            .iter()
            .filter(|document| document.contains(term))
            .count();
        // Integer rarity bonus in milli-points: rare terms outweigh
        // ubiquitous ones while scores stay integer and deterministic.
        1 + 1000 * (total + 1 - containing) / (total + 1)
    };
    let mut matches: Vec<_> = workflows
        .iter()
        .enumerate()
        .filter_map(|(index, workflow)| {
            let haystack = &documents[index];
            let lowered_id = workflow.id.to_ascii_lowercase().replace('.', " ");
            let mut score = 0_usize;
            for term in &terms {
                for variant in expand_term(term) {
                    if haystack.contains(&variant) {
                        let mut points = idf(&variant);
                        if lowered_id.contains(&variant) {
                            points *= 3;
                        }
                        if variant != *term {
                            points = points.max(1) / 2;
                        }
                        score += points;
                        break;
                    }
                }
            }
            if score == 0 {
                return None;
            }
            if let Some((_, runs, succeeded)) = success.iter().find(|(id, _, _)| id == &workflow.id)
            {
                if *runs > 0 {
                    // Demote flaky capabilities deterministically: scale by
                    // observed success, keeping one point so a tried
                    // capability never vanishes entirely.
                    let score_u64 = u64::try_from(score).unwrap_or(u64::MAX);
                    let scaled = score_u64.saturating_mul(*succeeded) / (*runs).max(1);
                    score = usize::try_from(scaled).unwrap_or(usize::MAX).max(1);
                }
            }
            Some(WorkflowMatch {
                workflow: workflow.id.clone(),
                score,
            })
        })
        .collect();
    matches.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.workflow.cmp(&right.workflow))
    });
    matches.truncate(limit);
    matches
}

/// Promotes a mined routine into a composable sub-workflow draft.
///
/// The draft carries the routine's op sequence with placeholder targets: the
/// caller re-binds semantics per use site. Values, selectors, and secrets are
/// never carried over — the signature that produced the routine never had
/// them.
#[must_use]
pub fn promote_routine_to_draft(namespace: &str, name: &str, routine: &LearnedRoutine) -> Workflow {
    let steps = routine
        .signature
        .iter()
        .enumerate()
        .map(|(index, signature)| {
            let mut parts = signature.split('|');
            let op = parts.next().unwrap_or("ui.wait").to_string();
            let semantic = parts.nth(1).unwrap_or("_");
            let target = if op.starts_with("ui.") && op != "ui.wait" {
                Some(crate::Target {
                    semantic: Some(format!("routine_{semantic}")),
                    ..crate::Target::default()
                })
            } else {
                None
            };
            crate::Step {
                id: format!("step_{}", index + 1),
                op,
                target,
                value: None,
                url: None,
                limit: None,
                timeout_ms: None,
                condition: None,
                iterations: None,
                destination: None,
                path: None,
            }
        })
        .collect();
    Workflow {
        version: crate::SUPPORTED_IR_VERSION.to_string(),
        id: format!("{namespace}.{name}"),
        description: format!(
            "Composed routine of {} steps observed in {} workflow(s)",
            routine.signature.len(),
            routine.occurrences.len()
        ),
        runtime: "browser".to_string(),
        inputs: BTreeMap::new(),
        steps,
        outputs: BTreeMap::new(),
        policy: None,
        preconditions: Vec::new(),
        success: Vec::new(),
        failure: Vec::new(),
        recovery: None,
        observed_apis: Vec::new(),
    }
}

fn step_signature(step: &Step) -> String {
    let role = step
        .target
        .as_ref()
        .and_then(|target| target.role.as_deref())
        .unwrap_or("_");
    let semantic = step
        .target
        .as_ref()
        .and_then(|target| target.semantic.as_deref())
        .unwrap_or("_");
    format!("{}|{}|{}", step.op, role, semantic)
}

fn tokens(text: &str) -> BTreeSet<String> {
    text.to_ascii_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| token.len() > 1)
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RecoveryPolicy, Step, Target};

    fn workflow(id: &str, description: &str, value: &str) -> Workflow {
        Workflow {
            version: "1.0".to_string(),
            id: id.to_string(),
            description: description.to_string(),
            runtime: "browser".to_string(),
            inputs: BTreeMap::new(),
            steps: vec![
                Step {
                    id: "search".to_string(),
                    op: "ui.fill".to_string(),
                    target: Some(Target {
                        semantic: Some("search_box".to_string()),
                        role: Some("textbox".to_string()),
                        ..Target::default()
                    }),
                    value: Some(value.to_string()),
                    url: None,
                    limit: None,
                    timeout_ms: None,
                    condition: None,
                    iterations: None,
                    destination: None,
                    path: None,
                },
                Step {
                    id: "submit".to_string(),
                    op: "ui.press".to_string(),
                    target: Some(Target {
                        semantic: Some("search_box".to_string()),
                        role: Some("textbox".to_string()),
                        ..Target::default()
                    }),
                    value: Some("Enter".to_string()),
                    url: None,
                    limit: None,
                    timeout_ms: None,
                    condition: None,
                    iterations: None,
                    destination: None,
                    path: None,
                },
            ],
            outputs: BTreeMap::new(),
            policy: None,
            preconditions: Vec::new(),
            success: Vec::new(),
            failure: Vec::new(),
            recovery: Some(RecoveryPolicy {
                max_attempts: 2,
                checkpoints: Vec::new(),
                capture_aria: true,
                capture_screenshot: true,
                vision_adapter: None,
            }),
            observed_apis: Vec::new(),
        }
    }

    #[test]
    fn repeated_sequences_become_value_free_routines() {
        let workflows = vec![
            workflow("shop.search", "Search a shop", "private one"),
            workflow("docs.search", "Search docs", "private two"),
        ];
        let routines = mine_routines(&workflows, 2, 4);
        assert_eq!(routines.len(), 1);
        assert_eq!(routines[0].occurrences.len(), 2);
        let encoded = serde_json::to_string(&routines).expect("serialize routines");
        assert!(!encoded.contains("private one"));
        assert!(!encoded.contains("private two"));
    }

    #[test]
    fn retrieval_prefers_id_matches() {
        let workflows = vec![
            workflow("shop.search", "Find products", "one"),
            workflow("docs.lookup", "Search product documentation", "two"),
        ];
        let matches = rank_workflows("shop search", &workflows, 5);
        assert_eq!(matches[0].workflow, "shop.search");
    }

    #[test]
    fn synonyms_bridge_query_vocabulary() {
        let workflows = vec![
            workflow("shop.search", "Find products", "one"),
            workflow("docs.lookup", "Search product documentation", "two"),
        ];
        let matches = rank_workflows("type to look up", &workflows, 5);
        assert!(
            matches.iter().any(|hit| hit.workflow == "shop.search"),
            "fill synonym should match fill workflows: {matches:?}"
        );
    }

    #[test]
    fn flaky_capabilities_are_demoted_but_kept() {
        let workflows = vec![
            workflow("shop.search", "Find products", "one"),
            workflow("docs.lookup", "Search product documentation", "two"),
        ];
        let success = vec![
            ("shop.search".to_string(), 10_u64, 1_u64),
            ("docs.lookup".to_string(), 10_u64, 10_u64),
        ];
        let matches = rank_workflows_weighted("search", &workflows, 5, &success);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].workflow, "docs.lookup");
        assert_eq!(matches[1].workflow, "shop.search");
    }

    #[test]
    fn routines_promote_to_value_free_drafts() {
        let workflows = vec![
            workflow("shop.search", "Search a shop", "private one"),
            workflow("docs.search", "Search docs", "private two"),
        ];
        let routines = mine_routines(&workflows, 2, 4);
        assert_eq!(routines.len(), 1);
        let draft = promote_routine_to_draft("common", "search_flow", &routines[0]);
        assert_eq!(draft.id, "common.search_flow");
        assert_eq!(draft.steps.len(), routines[0].signature.len());
        let encoded = serde_json::to_string(&draft).expect("serialize draft");
        assert!(!encoded.contains("private one"));
        crate::validate_workflow(&draft).expect("promoted draft validates");
    }
}
