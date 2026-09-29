//! Policy routing engine: deny, pin, budget, and redact MCP traffic.
//!
//! Rules come from `gateway.policies` and evaluate in file order; the first
//! matching rule wins. Evaluation is pure — [`PolicyEngine::evaluate`] never
//! mutates budget counters — so dry runs stay side-effect free. The gateway
//! calls [`PolicyEngine::consume`] once per allowed request to charge the
//! matching budget rule.
//!
//! This is traffic management (which upstream serves a call, at what rate,
//! with what fields), not identity: the `agentmesh-policy` crate owns
//! principal-based RBAC and human approvals. A future identity layer can feed
//! the authenticated principal into [`PolicyEngine::evaluate`] as an
//! additional match dimension.

use std::{
    collections::VecDeque,
    sync::Mutex,
    time::{Duration, Instant},
};

use agentmesh_config::{ConfigError, GatewayConfig, PolicyRule};
use agentmesh_protocol::{JsonRpcMessage, McpMethod};
use serde_json::Value;

/// Rolling window for hourly budgets.
const BUDGET_WINDOW: Duration = Duration::from_secs(3_600);
/// Replacement for redacted argument fields.
const REDACTED: &str = "[REDACTED]";
/// MCP method implied when a rule matches on `tool` without naming a method.
const TOOLS_CALL: &str = "tools/call";

/// One compiled rule with its route target resolved to a target index.
#[derive(Debug)]
struct CompiledRule {
    /// Glob matched against the `tools/call` tool name, when set.
    tool: Option<String>,
    /// MCP method label this rule applies to; `None` matches every method.
    method: Option<String>,
    /// Required gateway environment, when set.
    environment: Option<String>,
    /// Denial reason; presence rejects the call.
    deny: Option<String>,
    /// Pinned upstream target index, when set.
    route: Option<usize>,
    /// Hourly call budget, when set.
    budget_limit: Option<u64>,
    /// Dotted `arguments` paths to redact before forwarding.
    redact: Vec<String>,
}

/// Decision produced for one request.
#[derive(Debug, Clone)]
pub enum PolicyDecision {
    /// The request may proceed.
    Allow {
        /// Index of the matching rule, if any rule matched.
        rule: Option<usize>,
        /// Pinned upstream target index, when the rule pins one.
        route: Option<usize>,
        /// Argument paths to redact before forwarding.
        redact: Vec<String>,
    },
    /// The request must be rejected.
    Deny {
        /// Index of the matching rule.
        rule: usize,
        /// Human-readable reason, safe to expose.
        reason: String,
        /// True when a budget was exhausted rather than an explicit deny.
        quota: bool,
    },
}

/// Evaluation result plus the budget slot to charge on allow.
#[derive(Debug, Clone)]
pub struct PolicyEvaluation {
    /// Allow or deny with details.
    pub decision: PolicyDecision,
    /// Rule index whose budget must be charged when the call proceeds.
    pub budget_slot: Option<usize>,
}

/// First-match-wins policy engine built from gateway configuration.
#[derive(Debug)]
pub struct PolicyEngine {
    environment: String,
    upstream_names: Vec<String>,
    source: Vec<PolicyRule>,
    rules: Vec<CompiledRule>,
    budgets: Mutex<Vec<VecDeque<Instant>>>,
}

impl PolicyEngine {
    /// Compiles and validates the configured policies.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Validation`] when a `route` names an unknown
    /// upstream.
    pub fn new(config: &GatewayConfig) -> Result<Self, ConfigError> {
        let upstreams = config.effective_upstreams();
        let upstream_names: Vec<String> = upstreams
            .iter()
            .enumerate()
            .map(|(index, upstream)| upstream.effective_name(index))
            .collect();
        let mut rules = Vec::with_capacity(config.policies.len());
        for (index, rule) in config.policies.iter().enumerate() {
            let method = match (&rule.match_spec.tool, &rule.match_spec.method) {
                (_, Some(method)) => Some(method.clone()),
                (Some(_), None) => Some(TOOLS_CALL.to_string()),
                (None, None) => None,
            };
            let route = match &rule.route {
                Some(name) => Some(resolve_route(name, &upstream_names).ok_or_else(|| {
                    ConfigError::Validation(format!(
                        "gateway.policies[{index}].route names unknown upstream {name:?}; \
                         available: {}",
                        join_names(&upstream_names)
                    ))
                })?),
                None => None,
            };
            rules.push(CompiledRule {
                tool: rule.match_spec.tool.clone(),
                method,
                environment: rule.match_spec.environment.clone(),
                deny: rule.deny.clone(),
                route,
                budget_limit: rule.budget.as_ref().map(|budget| budget.calls_per_hour),
                redact: rule.redact.clone(),
            });
        }
        let slots = rules.len();
        Ok(Self {
            environment: config.environment.clone(),
            upstream_names,
            source: config.policies.clone(),
            rules,
            budgets: Mutex::new(vec![VecDeque::new(); slots]),
        })
    }

    /// Returns true when no policy rule is configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Returns the gateway environment this engine was built with.
    #[must_use]
    pub fn environment(&self) -> &str {
        &self.environment
    }

    /// Returns the resolved upstream name for a target index.
    #[must_use]
    pub fn upstream_name(&self, index: usize) -> Option<&str> {
        self.upstream_names.get(index).map(String::as_str)
    }

    /// Returns the original configured rule for display purposes.
    #[must_use]
    pub fn source_rule(&self, index: usize) -> Option<&PolicyRule> {
        self.source.get(index)
    }

    /// Evaluates the first matching rule without charging any budget.
    #[must_use]
    pub fn evaluate(&self, method: &str, tool: Option<&str>) -> PolicyEvaluation {
        for (index, rule) in self.rules.iter().enumerate() {
            if rule
                .environment
                .as_deref()
                .is_some_and(|environment| environment != self.environment)
            {
                continue;
            }
            if rule.method.as_deref().is_some_and(|name| name != method) {
                continue;
            }
            if let Some(pattern) = &rule.tool {
                let Some(name) = tool else { continue };
                if !glob_match(pattern, name) {
                    continue;
                }
            }
            if let Some(reason) = &rule.deny {
                let reason = if reason.trim().is_empty() {
                    "Denied by gateway policy.".to_string()
                } else {
                    reason.clone()
                };
                return PolicyEvaluation {
                    decision: PolicyDecision::Deny {
                        rule: index,
                        reason,
                        quota: false,
                    },
                    budget_slot: None,
                };
            }
            if let Some(limit) = rule.budget_limit {
                if self.budget_used(index) >= limit {
                    return PolicyEvaluation {
                        decision: PolicyDecision::Deny {
                            rule: index,
                            reason: format!(
                                "Hourly budget of {limit} calls exhausted for this policy rule."
                            ),
                            quota: true,
                        },
                        budget_slot: None,
                    };
                }
                return PolicyEvaluation {
                    decision: PolicyDecision::Allow {
                        rule: Some(index),
                        route: rule.route,
                        redact: rule.redact.clone(),
                    },
                    budget_slot: Some(index),
                };
            }
            return PolicyEvaluation {
                decision: PolicyDecision::Allow {
                    rule: Some(index),
                    route: rule.route,
                    redact: rule.redact.clone(),
                },
                budget_slot: None,
            };
        }
        PolicyEvaluation {
            decision: PolicyDecision::Allow {
                rule: None,
                route: None,
                redact: Vec::new(),
            },
            budget_slot: None,
        }
    }

    /// Charges one call against a budget rule's rolling hourly window.
    pub fn consume(&self, slot: usize) {
        let Ok(mut budgets) = self.budgets.lock() else {
            return;
        };
        if let Some(window) = budgets.get_mut(slot) {
            prune_window(window);
            window.push_back(Instant::now());
        }
    }

    /// Returns `(limit, remaining)` for a budget rule after pruning.
    #[must_use]
    pub fn budget_remaining(&self, slot: usize) -> Option<(u64, u64)> {
        let limit = self.rules.get(slot)?.budget_limit?;
        let used = self.budget_used(slot);
        Some((limit, limit.saturating_sub(used)))
    }

    /// Counts calls in the rolling window for a budget rule.
    fn budget_used(&self, slot: usize) -> u64 {
        let Ok(mut budgets) = self.budgets.lock() else {
            return 0;
        };
        let Some(window) = budgets.get_mut(slot) else {
            return 0;
        };
        prune_window(window);
        u64::try_from(window.len()).unwrap_or(u64::MAX)
    }
}

/// Renders the short policy label recorded in monitoring events.
#[must_use]
pub fn decision_label(engine: &PolicyEngine, decision: &PolicyDecision) -> String {
    match decision {
        PolicyDecision::Allow {
            rule,
            route,
            redact,
        } => {
            let mut label = match rule {
                Some(index) => format!("allow:rule{index}"),
                None => "allow".to_string(),
            };
            if let Some(target) = route.and_then(|index| engine.upstream_name(index)) {
                label.push_str(&format!("->{target}"));
            }
            if !redact.is_empty() {
                label.push_str(&format!("+redact{}", redact.len()));
            }
            label
        }
        PolicyDecision::Deny { rule, quota, .. } => {
            if *quota {
                format!("deny:rule{rule}:budget")
            } else {
                format!("deny:rule{rule}")
            }
        }
    }
}

/// Replaces dotted `arguments` paths with `[REDACTED]` on a `tools/call`
/// request. Missing paths are ignored. Returns the redaction count.
pub fn redact_arguments(message: &mut JsonRpcMessage, paths: &[String]) -> usize {
    let JsonRpcMessage::Request(request) = message else {
        return 0;
    };
    if !matches!(request.method, McpMethod::ToolsCall) {
        return 0;
    }
    let Some(params) = request.params.as_mut() else {
        return 0;
    };
    let Some(arguments) = params.get_mut("arguments") else {
        return 0;
    };
    paths
        .iter()
        .filter(|path| redact_path(arguments, path))
        .count()
}

/// Resolves a route target by upstream name or URL.
fn resolve_route(name: &str, upstream_names: &[String]) -> Option<usize> {
    let wanted = name.trim();
    upstream_names
        .iter()
        .position(|candidate| candidate == wanted)
}

/// Joins upstream names for validation errors.
fn join_names(names: &[String]) -> String {
    names
        .iter()
        .map(|name| format!("{name:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Drops budget timestamps older than the rolling window.
fn prune_window(window: &mut VecDeque<Instant>) {
    while window
        .front()
        .is_some_and(|instant| instant.elapsed() >= BUDGET_WINDOW)
    {
        window.pop_front();
    }
}

/// Sets one dotted object path to `[REDACTED]`; only object keys are
/// traversed in this version.
fn redact_path(root: &mut Value, path: &str) -> bool {
    let mut segments = path.split('.').filter(|segment| !segment.is_empty());
    let Some(last) = segments.next_back() else {
        return false;
    };
    let mut current = root;
    for segment in segments {
        match current {
            Value::Object(map) => match map.get_mut(segment) {
                Some(next) => current = next,
                None => return false,
            },
            _ => return false,
        }
    }
    match current {
        Value::Object(map) if map.contains_key(last) => {
            map.insert(last.to_string(), Value::String(REDACTED.to_string()));
            true
        }
        _ => false,
    }
}

/// Matches `text` against a glob where `*` spans any sequence and `?`
/// spans exactly one character.
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let (mut px, mut tx) = (0_usize, 0_usize);
    let (mut star, mut mark) = (None, 0_usize);
    while tx < text.len() {
        if px < pattern.len() && (pattern[px] == b'?' || pattern[px] == text[tx]) {
            px += 1;
            tx += 1;
        } else if px < pattern.len() && pattern[px] == b'*' {
            star = Some(px);
            px += 1;
            mark = tx;
        } else if let Some(position) = star {
            px = position + 1;
            mark += 1;
            tx = mark;
        } else {
            return false;
        }
    }
    while px < pattern.len() && pattern[px] == b'*' {
        px += 1;
    }
    px == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentmesh_config::{GatewayConfig, UpstreamConfig};

    fn gateway(policies_yaml: &str) -> GatewayConfig {
        let yaml = format!(
            "host: 127.0.0.1\nport: 8080\nenvironment: test\nupstreams:\n  - url: http://127.0.0.1:3001/mcp\n    name: github-prod\n    allow_insecure_http: true\n  - url: http://127.0.0.1:3002/mcp\n    name: github-free\n    allow_insecure_http: true\npolicies:\n{policies_yaml}\n"
        );
        let config: GatewayConfig =
            serde_yaml::from_str(&yaml).expect("test gateway section parses");
        config
    }

    #[test]
    fn glob_supports_stars_and_questions() {
        assert!(glob_match("github.*", "github.get_issue"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("stripe.?efund", "stripe.refund"));
        assert!(!glob_match("github.*", "gitlab.get_issue"));
        assert!(!glob_match("abc", "abcd"));
        assert!(glob_match("a*b*c", "axbyc"));
        assert!(!glob_match("a*b", "axc"));
    }

    #[test]
    fn first_match_wins_and_unmatched_calls_pass() {
        let config = gateway(
            "  - match: { tool: \"github.*\" }\n    route: github-free\n  - match: { tool: \"*\" }\n    deny: Blocked.\n",
        );
        let engine = PolicyEngine::new(&config).expect("engine builds");
        let evaluation = engine.evaluate(TOOLS_CALL, Some("github.get_issue"));
        let PolicyDecision::Allow { rule, route, .. } = evaluation.decision else {
            panic!("first rule allows with a route");
        };
        assert_eq!(rule, Some(0));
        assert_eq!(route, Some(1));
        assert_eq!(evaluation.budget_slot, None);
        let evaluation = engine.evaluate(TOOLS_CALL, Some("other.tool"));
        assert!(matches!(
            evaluation.decision,
            PolicyDecision::Deny { rule: 1, .. }
        ));
        let evaluation = engine.evaluate("resources/read", None);
        assert!(matches!(
            evaluation.decision,
            PolicyDecision::Allow { rule: None, .. }
        ));
    }

    #[test]
    fn environment_mismatch_skips_the_rule() {
        let config = gateway(
            "  - match: { tool: \"stripe.refund\", environment: development }\n    deny: No refunds in dev.\n",
        );
        let engine = PolicyEngine::new(&config).expect("engine builds");
        let evaluation = engine.evaluate(TOOLS_CALL, Some("stripe.refund"));
        assert!(matches!(
            evaluation.decision,
            PolicyDecision::Allow { rule: None, .. }
        ));
    }

    #[test]
    fn unknown_route_rejects_the_configuration() {
        let config = gateway("  - match: { tool: \"github.*\" }\n    route: missing\n");
        let error = PolicyEngine::new(&config).expect_err("unknown route fails");
        assert!(error.to_string().contains("missing"));
    }

    #[test]
    fn budget_exhausts_after_its_limit() {
        let config =
            gateway("  - match: { tool: \"maps.*\" }\n    budget: { calls_per_hour: 2 }\n");
        let engine = PolicyEngine::new(&config).expect("engine builds");
        for _ in 0..2 {
            let evaluation = engine.evaluate(TOOLS_CALL, Some("maps.search"));
            let slot = evaluation.budget_slot.expect("budget slot");
            assert!(!matches!(evaluation.decision, PolicyDecision::Deny { .. }));
            engine.consume(slot);
        }
        assert_eq!(engine.budget_remaining(0), Some((2, 0)));
        let evaluation = engine.evaluate(TOOLS_CALL, Some("maps.search"));
        let PolicyDecision::Deny { quota, .. } = evaluation.decision else {
            panic!("third call exceeds the budget");
        };
        assert!(quota);
    }

    #[test]
    fn redact_replaces_nested_fields_and_counts() {
        let mut message = agentmesh_protocol::decode_message(
            br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"crm.create","arguments":{"customer":{"ssn":"123","name":"Ada"},"plain":true}}}"#,
            agentmesh_protocol::ProtocolLimits::default(),
        )
        .expect("message decodes");
        let count = redact_arguments(
            &mut message,
            &["customer.ssn".to_string(), "missing.path".to_string()],
        );
        assert_eq!(count, 1);
        let JsonRpcMessage::Request(request) = &message else {
            panic!("request survives");
        };
        let arguments = request.params.as_ref().expect("params survive");
        assert_eq!(arguments["arguments"]["customer"]["ssn"], "[REDACTED]");
        assert_eq!(arguments["arguments"]["customer"]["name"], "Ada");
    }

    #[test]
    fn upstream_name_resolution_prefers_configured_names() {
        let upstream = UpstreamConfig {
            name: Some("github-prod".to_string()),
            ..UpstreamConfig::default()
        };
        assert_eq!(upstream.effective_name(0), "github-prod");
        assert_eq!(UpstreamConfig::default().effective_name(3), "upstream-3");
    }
}
