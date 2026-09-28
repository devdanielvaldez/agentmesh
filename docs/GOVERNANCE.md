# Identity, governance, limits, and tasks

AgentMesh keeps authentication, authorization, contextual policy, admission limits, and long-running
task ownership as separate decisions. The same contracts run in a local binary or against immutable
cloud control-plane snapshots.

```text
credential → authentication → normalized principal
                                  │
request + resource ───────────────┼→ RBAC authorization
                                  │          │
context + risk + metadata ────────┼──────────┴→ contextual policy
                                  │                    │
                                  │          allow / deny / approval
                                  ▼                    │
                         scoped admission limits ◄─────┘
                                  │
                                  ▼
                              MCP call
                                  │ optional long-running task
                                  ▼
                       owner + backend affinity map
```

## Authentication

`agentmesh-authn` accepts exactly one presented mechanism and runs a bounded pluggable verifier
chain. Successful verifiers produce a tenant-scoped normalized principal, authentication strength,
trusted role assertions, and sanitized evidence. Authentication never implies authorization.

The built-in standalone provider stores SHA-256 API-key digests rather than plaintext, compares
digests without data-dependent early exit, supports expiry and disabling, and validates every
identity field. Credential and key-record `Debug` output is redacted. Bearer, mTLS, OIDC, and
workload mechanisms are explicit variants handled by verifier adapters; network refresh and trust
validation stay outside the synchronous request path.

## RBAC authorization

`agentmesh-authz` compiles permissions, roles, and bindings into an immutable organization snapshot.
Bindings may apply across namespaces or to one namespace and may separately constrain delegated
users. Every decision verifies both principal and resource scope before matching.

Explicit deny always wins over allow. Missing allow defaults to deny. Decisions expose only a stable
reason and matched rule and are cached by policy revision, organization, namespace, principal,
action, resource, labels, and delegated user. The same evaluation method filters visible tools and
authorizes invocation, preventing catalog/call inconsistencies.

## Contextual policy and approvals

`agentmesh-policy` evaluates rules ordered by priority and stable ID. Match inputs include action,
trusted roles, environment, data classification, risk, authentication strength, UTC time window,
and sanitized argument metadata. Raw tool arguments are not retained by the policy module.

Rules produce `allow`, `deny`, or `require_approval`, plus stable reasons and bounded obligations.
No match and tenant mismatch fail closed. Equal-precedence identical matches with conflicting effects
are rejected during compilation. Bounded batch simulation supports dry-run rollouts without changing
runtime state.

The standalone approval workflow stores opaque requests with expiry and enforces separation of
duties: the requester cannot approve their own operation. Production persistence and audit delivery
will use the storage and audit modules while preserving this contract.

## Rate, concurrency, and quota enforcement

`agentmesh-rate-limit` atomically applies all three controls before admitting work:

- token-bucket rate with millitoken precision;
- RAII concurrency leases released on cancellation or panic;
- fixed-window request or cost quota.

Keys always include tenant scope, dimension, normalized subject, and policy revision. Supported
dimensions are tenant, principal, validated IP, service, and capability. Counter cardinality is
bounded; inactive entries can be purged in batches; snapshots expose only low-cardinality usage.
The in-memory implementation is the standalone backend. A future Redis adapter can implement strict
global enforcement without changing request semantics.

## Long-running tasks

`agentmesh-tasks` replaces upstream task identifiers with opaque AgentMesh IDs and records the owner,
tenant, backend endpoint, status, expiry, trace reference, and optimistic revision. Backend IDs are
redacted from formatting and returned only to the task/proxy adapter.

Get, transition, and cancel operations each re-check tenant and owner (or explicit tenant-admin)
access. The state machine rejects stale revisions and terminal-state changes. Expired tasks are
hidden before bounded cleanup, and sanitized transitions are available for telemetry/audit.

## Integrated behavior

The governance integration test exercises the entire decision chain: an API key creates a principal,
RBAC authorizes a tool, contextual policy requires independent approval, scoped admission consumes a
rate/quota/concurrency lease, and the accepted call creates an owner-protected task mapping. Failure
at any stage prevents all downstream work.
