# MCPMesh

> **The Service Mesh for Model Context Protocol.**

MCPMesh is an open-source infrastructure layer for discovering, routing, securing, observing, balancing, and governing Model Context Protocol servers at scale.

Instead of requiring every AI application or agent to connect directly to individual MCP servers, applications connect to MCPMesh.

```text
AI Agents
    │
    ▼
┌───────────────────────┐
│        MCPMesh        │
│                       │
│ Discovery             │
│ Routing               │
│ Load Balancing        │
│ Authentication        │
│ Authorization         │
│ Policies              │
│ Rate Limiting         │
│ Circuit Breakers      │
│ Observability         │
│ Auditing              │
│ Caching               │
│ Health Checking       │
└───────────┬───────────┘
            │
    ┌───────┼────────┬───────────┐
    ▼       ▼        ▼           ▼
 GitHub    Slack   PostgreSQL   Internal
   MCP      MCP       MCP         MCP
```

---

# 1. Vision

Model Context Protocol provides a standard way for AI applications and agents to communicate with external tools, resources, and services.

A simple environment may contain:

```text
Agent
  │
  ├── GitHub MCP
  ├── PostgreSQL MCP
  └── Slack MCP
```

This works well at small scale.

The problem changes when an organization has:

```text
500 AI Agents

100 MCP Servers

2,000 Tools

Multiple Teams

Multiple Environments

Multiple Regions

Thousands of Requests per Second
```

Now several infrastructure problems appear:

- How does an agent discover a tool?
- Which MCP server should receive the request?
- What happens if that server is unavailable?
- How are multiple replicas balanced?
- How do we prevent an agent from calling unauthorized tools?
- How are credentials handled?
- How do we rate-limit agents?
- How do we audit tool calls?
- How do we detect unhealthy MCP servers?
- How do we observe latency?
- How do we trace a request across multiple MCP servers?
- How do we prevent a failing server from affecting the entire system?
- How do we route production and development traffic differently?
- How do we enforce organization-wide policies?
- How do we discover new MCP servers automatically?

MCPMesh exists to solve these problems.

---

# 2. Core Thesis

The fundamental idea is:

> MCP servers should be treated as discoverable, routable, observable, policy-controlled services.

MCPMesh introduces an infrastructure layer between AI applications and MCP servers.

```text
WITHOUT MCPMESH

Agent
 ├── Github MCP
 ├── Slack MCP
 ├── Jira MCP
 ├── PostgreSQL MCP
 ├── AWS MCP
 └── Internal MCP
```

With MCPMesh:

```text
WITH MCPMESH

Agent
  │
  ▼
MCPMesh
  │
  ├── Github MCP
  ├── Slack MCP
  ├── Jira MCP
  ├── PostgreSQL MCP
  ├── AWS MCP
  └── Internal MCP
```

Applications only need to know about MCPMesh.

---

# 3. Positioning

MCPMesh should not attempt to replace MCP.

It should implement and extend the operational infrastructure around MCP.

The conceptual relationship is:

```text
HTTP       → Envoy / NGINX / Service Mesh

Containers → Kubernetes

Services   → Istio

MCP        → MCPMesh
```

Possible tagline:

> **The Service Mesh for AI Tools.**

Alternative:

> **Secure, route and observe MCP at scale.**

Another:

> **Infrastructure for production MCP.**

---

# 4. Design Principles

MCPMesh should follow these principles.

## MCP Native

MCPMesh must understand MCP semantics.

It should understand concepts such as:

```text
tools
resources
prompts
tasks
capabilities
protocol versions
MCP methods
MCP names
```

It should not simply be a generic HTTP reverse proxy.

---

## Transparent

Existing MCP clients should require minimal changes.

Ideally:

```text
Before:

https://github-mcp.company.com/mcp

After:

https://mesh.company.com/mcp/github
```

or:

```text
https://mesh.company.com/mcp
```

with routing determined automatically.

---

## Provider Agnostic

MCPMesh must not depend on:

```text
OpenAI
Anthropic
Google
Meta
Vercel
```

Any MCP-compatible client should be able to use it.

---

## Server Agnostic

MCP servers can be implemented in:

```text
Rust
TypeScript
Python
Go
Java
C#
Kotlin
Ruby
PHP
```

MCPMesh should not care.

---

## Secure by Default

The default architecture should assume:

```text
Agents are not fully trusted.

MCP servers are not automatically trusted.

External metadata is not automatically trusted.

Credentials must never be casually forwarded.

Every sensitive action should be auditable.
```

---

## High Performance

MCPMesh sits on the request path.

Therefore:

```text
low latency
high concurrency
low memory usage
predictable performance
```

are essential.

This is one of the main reasons for implementing the core in Rust.

---

# 5. High-Level Architecture

```text
                      MCP CLIENTS

              ┌──────────┼──────────┐
              ▼          ▼          ▼

           AI Agent   AI Agent   AI App

              \          |          /
               \         |         /
                ─────────┼─────────
                         ▼

                ┌──────────────────┐
                │                  │
                │     MCPMesh      │
                │                  │
                │   Data Plane     │
                │                  │
                └────────┬─────────┘
                         │
             ┌───────────┼────────────┐
             ▼           ▼            ▼

          MCP Server  MCP Server   MCP Server
          Github      PostgreSQL   Internal
```

MCPMesh itself should contain two major layers:

```text
CONTROL PLANE

and

DATA PLANE
```

---

# 6. Control Plane

The Control Plane manages configuration and desired state.

```text
┌─────────────────────────────────────┐
│          MCPMesh Control Plane      │
│                                     │
│ Registry                            │
│ Service Discovery                   │
│ Configuration                       │
│ Policies                            │
│ Identity                            │
│ Certificates                        │
│ Routing Rules                       │
│ Health State                        │
│ Secrets References                  │
│ Metrics Configuration               │
│ Cluster Membership                  │
└──────────────────┬──────────────────┘
                   │
                   ▼
             Data Plane
```

The Control Plane should not normally process MCP tool traffic directly.

Its responsibility is configuration.

---

# 7. Data Plane

The Data Plane processes MCP requests.

```text
Agent
  │
  ▼
MCPMesh Data Plane
  │
  ├── Authentication
  ├── Authorization
  ├── Policy Evaluation
  ├── Rate Limiting
  ├── Routing
  ├── Load Balancing
  ├── Retry
  ├── Circuit Breaker
  ├── Observability
  └── Proxy
        │
        ▼
     MCP Server
```

The Data Plane should be optimized for extremely fast request processing.

---

# 8. Request Lifecycle

A request could follow this lifecycle:

```text
MCP Request
     │
     ▼
Protocol Validation
     │
     ▼
Authentication
     │
     ▼
Identity Resolution
     │
     ▼
Policy Evaluation
     │
     ▼
Rate Limit
     │
     ▼
Tool Resolution
     │
     ▼
Server Discovery
     │
     ▼
Route Selection
     │
     ▼
Load Balancing
     │
     ▼
Circuit Breaker Check
     │
     ▼
Upstream Request
     │
     ▼
Response Validation
     │
     ▼
Telemetry
     │
     ▼
MCP Response
```

Every step should produce telemetry.

---

# 9. MCP Server Registry

MCPMesh needs a registry containing every known MCP server.

Example:

```yaml
apiVersion: mesh.mcp/v1
kind: MCPServer

metadata:
  name: github

spec:
  endpoints:
    - https://github-mcp-01.internal/mcp
    - https://github-mcp-02.internal/mcp
    - https://github-mcp-03.internal/mcp

  protocol:
    transport: streamable-http

  health:
    interval: 10s
    timeout: 2s

  tags:
    environment: production
    team: engineering
```

Register:

```bash
mcpmesh apply -f github.yaml
```

Query:

```bash
mcpmesh get servers
```

Output:

```text
NAME          ENDPOINTS    STATUS       TOOLS

github        3            Healthy      18
postgres      2            Healthy      12
slack         2            Degraded     24
jira          1            Healthy      31
```

---

# 10. Automatic Discovery

MCPMesh should support multiple discovery mechanisms.

## Static

Configured manually:

```yaml
servers:
  github:
    url: https://github-mcp.internal/mcp
```

---

## DNS

Example:

```text
_mcp._tcp.company.internal
```

---

## Kubernetes

Discover MCP servers from:

```text
Services
Pods
Annotations
Custom Resources
```

Example annotation:

```yaml
metadata:
  annotations:
    mcpmesh.io/enabled: "true"
```

---

## Docker

Discover containers with labels:

```text
mcpmesh.enabled=true
```

---

## Registry API

Servers can register themselves.

```http
POST /api/v1/servers/register
```

---

# 11. Capability Discovery

MCPMesh should know which capabilities every server exposes.

For example:

```text
github-mcp

tools:
  repository.read
  repository.create
  issue.create
  pull_request.create
  pull_request.merge
```

Registry:

```text
Capability Registry

github.create_pull_request
      │
      ├── github-mcp-01
      ├── github-mcp-02
      └── github-mcp-03
```

This enables capability-based routing.

---

# 12. Tool Registry

MCPMesh maintains a global tool catalog.

Example:

```bash
mcpmesh get tools
```

Output:

```text
TOOL                         SERVER       STATUS

github.create_issue          github       Ready
github.create_pull_request   github       Ready
github.merge_pull_request    github       Ready

postgres.query               postgres     Ready
postgres.execute             postgres     Ready

slack.send_message           slack        Ready
```

---

# 13. Namespaces

Tool names should be namespaced.

Instead of:

```text
create
delete
query
send
```

MCPMesh exposes:

```text
github.issue.create

github.pull_request.create

postgres.query

slack.message.send
```

This avoids tool collisions.

---

# 14. Capability Addressing

MCPMesh can introduce logical capability addresses.

Example:

```text
mcp://github/pull-request/create
```

or:

```text
tool://github/pull_request/create
```

The application does not need to know the physical MCP server.

```text
tool://github/pull_request/create

                ↓

             MCPMesh

                ↓

       github-mcp-02.internal
```

---

# 15. Smart Tool Resolution

Eventually MCPMesh could resolve generic capabilities.

For example:

```text
tool://email/send
```

Available providers:

```text
gmail-mcp
outlook-mcp
smtp-mcp
```

Routing rules determine which one should be used.

Example:

```yaml
routing:

  email.send:

    production:
      provider: gmail

    internal:
      provider: smtp
```

---

# 16. Routing Engine

Routing is one of the central components.

A routing decision can use:

```text
MCP method
MCP name
Tool
Resource
Agent identity
User identity
Organization
Environment
Region
Server health
Latency
Server load
Policy
Version
Cost
Tags
```

---

# 17. Header-Based Routing

Modern MCP requests expose routing information in HTTP headers.

MCPMesh should exploit this directly.

Conceptually:

```text
Mcp-Method: tools/call

Mcp-Name: github.create_issue
```

Routing:

```text
Mcp-Name
   │
   ▼
Tool Registry
   │
   ▼
github service
   │
   ▼
healthy replica
```

This avoids unnecessary deep JSON parsing for many routing decisions.

---

# 18. Routing Rules

Example:

```yaml
apiVersion: mesh.mcp/v1
kind: MCPRoute

metadata:
  name: github-routing

spec:

  match:
    tool: github.*

  destination:
    service: github

  strategy:
    type: least-connections
```

Another:

```yaml
match:
  tool: database.query

destination:
  service: postgres-readonly
```

While:

```yaml
match:
  tool: database.execute

destination:
  service: postgres-primary
```

---

# 19. Environment Routing

Example:

```yaml
routes:

  - match:
      environment: development
      tool: database.*

    destination:
      service: postgres-development

  - match:
      environment: production
      tool: database.*

    destination:
      service: postgres-production
```

---

# 20. Geographic Routing

Example:

```text
Agent in Europe
       │
       ▼
MCPMesh
       │
       ▼
EU MCP Cluster
```

while:

```text
Agent in USA
       │
       ▼
MCPMesh
       │
       ▼
US MCP Cluster
```

Useful for:

```text
latency
data residency
privacy
compliance
```

---

# 21. Load Balancing

Multiple replicas should be supported.

```text
github service

├── github-mcp-01
├── github-mcp-02
└── github-mcp-03
```

Strategies:

```text
round-robin
least-connections
least-latency
random
weighted
consistent-hash
adaptive
```

Configuration:

```yaml
loadBalancing:
  strategy: least-latency
```

---

# 22. Weighted Routing

Useful for migrations.

```yaml
destinations:

  - server: github-v1
    weight: 90

  - server: github-v2
    weight: 10
```

This enables:

```text
90% → v1
10% → v2
```

Then:

```text
50 / 50
```

Eventually:

```text
0 / 100
```

---

# 23. Canary MCP Servers

MCPMesh should support canary deployments.

```text
github-mcp-v1

vs

github-mcp-v2
```

Example:

```yaml
canary:
  percentage: 5
```

Metrics compare:

```text
error rate
latency
tool failures
response size
timeouts
```

If v2 becomes unhealthy:

```text
automatic rollback
```

---

# 24. Health Checking

MCPMesh should continuously evaluate server health.

States:

```text
HEALTHY
DEGRADED
UNHEALTHY
DRAINING
DISABLED
```

Example:

```bash
mcpmesh get servers
```

```text
github-01    Healthy
github-02    Healthy
github-03    Unhealthy
```

Requests stop going to unhealthy instances.

---

# 25. Passive Health Detection

Health should not depend exclusively on explicit probes.

MCPMesh can observe:

```text
timeouts
5xx
protocol errors
invalid responses
latency
connection failures
```

Example:

```text
github-mcp-02

last 100 requests

38 failures

↓

DEGRADED
```

---

# 26. Circuit Breakers

If a server repeatedly fails:

```text
CLOSED
   │
   │ failures
   ▼
OPEN
   │
   │ wait
   ▼
HALF-OPEN
   │
   ├── success → CLOSED
   │
   └── failure → OPEN
```

Configuration:

```yaml
circuitBreaker:

  failureThreshold: 5

  window: 30s

  openDuration: 20s

  halfOpenRequests: 3
```

---

# 27. Retries

Retries should only happen when semantically safe.

Configuration:

```yaml
retry:

  attempts: 3

  backoff: exponential

  retryOn:
    - timeout
    - connection_error
    - unavailable
```

MCPMesh should avoid blindly retrying mutating tools.

Example:

```text
payment.create
```

must not accidentally execute twice.

Therefore tool metadata should support:

```yaml
semantics:
  idempotent: false
```

---

# 28. Timeouts

Timeouts can exist at multiple levels.

```yaml
timeouts:

  connect: 2s

  request: 30s

  idle: 60s
```

Per-tool:

```yaml
tools:

  github.search:
    timeout: 10s

  research.deep_search:
    timeout: 5m
```

---

# 29. Long-Running Tasks

MCPMesh should support MCP Tasks.

Architecture:

```text
Agent
  │
  ▼
tools/call
  │
  ▼
MCPMesh
  │
  ▼
MCP Server
  │
  ▼
Task Handle
```

Then:

```text
tasks/get

tasks/update

tasks/cancel
```

MCPMesh should preserve routing between the logical task and its backend.

---

# 30. Task Registry

For observability MCPMesh can maintain:

```text
Task ID
Server
Tool
Agent
Started
Status
Duration
Trace
```

Example:

```bash
mcpmesh get tasks
```

```text
TASK        TOOL                SERVER       STATUS

task-821    research.deep      research-2   RUNNING
task-822    github.review      github-1     COMPLETE
```

---

# 31. Authentication

MCPMesh should authenticate callers before routing sensitive requests.

Supported identity mechanisms could include:

```text
OAuth 2.x
JWT
API Keys
mTLS
Service Accounts
OIDC
Workload Identity
```

Identity becomes:

```text
Principal
```

Example:

```text
principal:

type: agent

id: coding-agent-82

organization: acme

team: engineering

environment: production
```

---

# 32. Authorization

Authentication answers:

> Who are you?

Authorization answers:

> What can you do?

Example:

```yaml
apiVersion: mesh.mcp/v1
kind: MCPPolicy

metadata:
  name: developer-agent

spec:

  subject:
    role: developer

  allow:
    - github.repository.read
    - github.pull_request.create
    - postgres.read

  deny:
    - github.repository.delete
    - postgres.drop
    - production.deploy
```

---

# 33. Policy Engine

Policy evaluation:

```text
Agent
   │
   ▼
MCP Request
   │
   ▼
Identity
   │
   ▼
Policy Engine
   │
   ├── ALLOW
   ├── DENY
   └── REQUIRE_APPROVAL
```

---

# 34. Human Approval

Sensitive tools can require human approval.

```yaml
policy:

  tool: production.deploy

  effect: require-approval
```

Flow:

```text
Agent
   │
   ▼
production.deploy
   │
   ▼
MCPMesh
   │
   ▼
WAITING_APPROVAL
```

Dashboard:

```text
Agent: coding-agent-28

requests:

production.deploy

Environment:
production

Commit:
8a28bc

[ APPROVE ]

[ DENY ]
```

---

# 35. Risk Classification

Tools can have risk levels.

```text
LOW

MEDIUM

HIGH

CRITICAL
```

Example:

```yaml
tools:

  github.repository.read:
    risk: low

  github.pull_request.create:
    risk: medium

  database.write:
    risk: high

  database.drop:
    risk: critical
```

Policies can use these values.

---

# 36. Rate Limiting

Limits can apply to:

```text
agent
user
organization
tool
server
IP
API key
namespace
```

Example:

```yaml
rateLimit:

  subject:
    agent: research-agent

  tool: github.search

  requests:
    limit: 100
    window: 1m
```

---

# 37. Concurrency Limits

Example:

```yaml
concurrency:

  tool: browser.open

  maxConcurrent: 20
```

When exceeded:

```text
queue

or

reject
```

---

# 38. Quotas

Example:

```yaml
quota:

  organization: acme

  daily:
    requests: 100000

  monthly:
    requests: 2000000
```

Future versions could track monetary costs.

---

# 39. Credential Isolation

MCPMesh must not blindly forward credentials.

Conceptually:

```text
Agent Token

    ≠

MCP Server Token

    ≠

Downstream API Token
```

Credentials should remain bound to their intended resource.

---

# 40. Credential Broker

Future MCPMesh versions could include a credential broker.

```text
Agent
  │
  ▼
MCPMesh
  │
  ▼
Credential Broker
  │
  ▼
Upstream MCP
```

The agent does not necessarily receive the upstream credential.

---

# 41. Secret Providers

Possible integrations:

```text
HashiCorp Vault
AWS Secrets Manager
Azure Key Vault
Google Secret Manager
Kubernetes Secrets
Environment Variables
1Password
```

MCPMesh stores references rather than plaintext secrets whenever possible.

---

# 42. SSRF Protection

MCPMesh should protect discovery and OAuth-related requests.

Default production rules should include:

```text
HTTPS required

block loopback

block private ranges

block link-local

block cloud metadata endpoints

validate redirects

DNS re-resolution protection
```

Exceptions can be explicitly configured for internal deployments.

---

# 43. Egress Policies

Example:

```yaml
egress:

  github-mcp:

    allow:
      - api.github.com

    deny:
      - "*"
```

This prevents compromised servers from communicating arbitrarily.

---

# 44. Tool Allowlisting

Organizations can explicitly approve tools.

```yaml
allowTools:

  - github.search
  - github.pull_request.create
  - slack.message.send
```

Everything else:

```text
DENY
```

---

# 45. Tool Pinning

Tool definitions can change.

MCPMesh should optionally pin:

```text
tool name
description
input schema
output schema
server identity
version
hash
```

Example:

```text
github.pull_request.create

SHA256:
abc812...
```

If definition changes:

```text
EXPECTED

abc812

ACTUAL

82bc91

↓

QUARANTINE
```

---

# 46. Tool Integrity

MCPMesh can calculate a canonical hash for tool definitions.

```text
Tool Manifest
      │
      ▼
Canonical Representation
      │
      ▼
SHA-256
```

Changes become visible and auditable.

---

# 47. Quarantine

Suspicious servers can enter:

```text
QUARANTINED
```

No production traffic reaches them until reviewed.

Reasons:

```text
tool schema changed

identity changed

certificate changed

unexpected capabilities

excessive errors

security policy violation
```

---

# 48. Audit Log

Every important event should be recorded.

Example:

```json
{
  "timestamp": "2026-09-28T14:22:18Z",
  "agent": "coding-agent-82",
  "organization": "acme",
  "server": "github",
  "tool": "github.pull_request.create",
  "decision": "ALLOW",
  "latency_ms": 281,
  "status": "SUCCESS",
  "trace_id": "82abc19"
}
```

---

# 49. Immutable Audit Mode

Enterprise deployments could write audit logs to:

```text
S3 Object Lock

WORM storage

SIEM

external log service
```

to make modification difficult.

---

# 50. Observability

MCPMesh should expose three observability pillars:

```text
Metrics

Logs

Traces
```

---

# 51. Metrics

Example metrics:

```text
mcp_requests_total

mcp_request_duration_seconds

mcp_errors_total

mcp_active_requests

mcp_server_health

mcp_tool_calls_total

mcp_policy_denials_total

mcp_rate_limit_total

mcp_circuit_breaker_state

mcp_task_duration_seconds
```

---

# 52. Prometheus

Expose:

```text
/metrics
```

for Prometheus.

Example:

```text
mcp_requests_total{
  server="github",
  tool="create_issue",
  status="success"
}
```

---

# 53. Distributed Tracing

Use OpenTelemetry.

Example trace:

```text
Agent Request
     │
     ▼
MCPMesh
     │
     ├── authentication
     │
     ├── policy
     │
     ├── routing
     │
     └── upstream
            │
            ▼
       Github MCP
            │
            ▼
       Github API
```

All should share:

```text
trace_id
```

---

# 54. W3C Trace Context

MCPMesh should propagate:

```text
traceparent

tracestate

baggage
```

where supported.

This enables complete distributed traces.

---

# 55. Logs

Structured JSON logs.

Example:

```json
{
  "level": "info",
  "event": "tool.call",
  "tool": "github.search",
  "server": "github-02",
  "latency_ms": 182,
  "status": 200
}
```

---

# 56. Dashboard

A Next.js dashboard can provide:

```text
┌───────────────────────────────────────────┐
│ MCPMesh                                   │
├───────────────────────────────────────────┤
│                                           │
│ Servers            82                     │
│ Healthy            79                     │
│ Degraded            2                     │
│ Unhealthy           1                     │
│                                           │
│ Tools              842                    │
│                                           │
│ Requests / sec     12,821                 │
│                                           │
│ P95 latency         82ms                   │
│ Error rate          0.21%                  │
│                                           │
└───────────────────────────────────────────┘
```

---

# 57. Topology View

Visualize:

```text
                  coding-agents
                       │
                       ▼
                     Mesh
                  /    |    \
                 /     |     \
                ▼      ▼      ▼

             Github  Jira   PostgreSQL
               │             │
             3 pods         2 pods
```

---

# 58. Traffic View

Display live traffic:

```text
github.search

1,821 req/min

P50  28ms
P95  82ms
P99  182ms

Error 0.2%
```

---

# 59. Security Dashboard

Display:

```text
Policy denials

Rate limit events

Unauthorized calls

Quarantined servers

Tool definition changes

Authentication failures

Approval requests
```

---

# 60. CLI

CLI:

```text
mcpmesh
```

Examples:

```bash
mcpmesh get servers

mcpmesh get tools

mcpmesh get routes

mcpmesh get policies

mcpmesh get tasks

mcpmesh get agents
```

---

# 61. Describe

```bash
mcpmesh describe server github
```

Example:

```text
Name:
github

Status:
Healthy

Replicas:
3

Tools:
18

Requests:
281,821

P95:
72ms

Errors:
0.18%

Protocol:
2026-07-28
```

---

# 62. Apply

Declarative configuration:

```bash
mcpmesh apply -f mesh.yaml
```

Example:

```yaml
apiVersion: mesh.mcp/v1

kind: MCPServer

metadata:
  name: github

spec:

  endpoints:
    - https://github-01.internal/mcp
    - https://github-02.internal/mcp

  loadBalancing:
    strategy: least-latency

  circuitBreaker:
    failureThreshold: 5

  health:
    interval: 10s
```

---

# 63. Test

Useful command:

```bash
mcpmesh test server github
```

Could verify:

```text
Connectivity       PASS

Protocol           PASS

Authentication     PASS

Tool discovery     PASS

Schemas            PASS

Latency            28ms
```

---

# 64. Doctor

```bash
mcpmesh doctor
```

Checks:

```text
Control plane

Database

Certificates

Routes

DNS

Servers

Policies

Telemetry

Configuration
```

---

# 65. Traffic Inspection

Development mode:

```bash
mcpmesh traffic
```

Output:

```text
14:22:18

agent-82
→ github.search
→ github-mcp-02
→ 200
→ 82ms
```

Filter:

```bash
mcpmesh traffic \
  --tool github.search
```

---

# 66. Architecture in Rust

The main implementation should use Rust.

Recommended stack:

```text
Rust

Tokio

Axum

Tower

Serde

RMCP

SQLx

Clap

Tracing

OpenTelemetry
```

---

# 67. Why Rust

MCPMesh is fundamentally:

```text
network proxy

routing engine

policy engine

high concurrency server

telemetry collector

service discovery system
```

Rust provides:

```text
high performance

memory safety

low runtime overhead

excellent async support

single binary deployment

strong concurrency guarantees
```

---

# 68. Rust Workspace

Recommended repository:

```text
mcpmesh/

├── crates/
│
│   ├── mcpmesh-core/
│   ├── mcpmesh-protocol/
│   ├── mcpmesh-proxy/
│   ├── mcpmesh-router/
│   ├── mcpmesh-discovery/
│   ├── mcpmesh-registry/
│   ├── mcpmesh-policy/
│   ├── mcpmesh-auth/
│   ├── mcpmesh-rate-limit/
│   ├── mcpmesh-circuit-breaker/
│   ├── mcpmesh-health/
│   ├── mcpmesh-cache/
│   ├── mcpmesh-telemetry/
│   ├── mcpmesh-config/
│   ├── mcpmesh-control-plane/
│   ├── mcpmesh-data-plane/
│   └── mcpmesh-cli/
│
├── dashboard/
│
├── sdk/
│   ├── typescript/
│   ├── python/
│   └── rust/
│
├── examples/
│
├── deployments/
│   ├── docker/
│   ├── kubernetes/
│   └── helm/
│
├── docs/
│
├── Cargo.toml
│
└── README.md
```

---

# 69. mcpmesh-core

Contains fundamental domain types.

Example:

```rust
pub struct McpServer {
    pub id: ServerId,
    pub name: String,
    pub endpoints: Vec<Endpoint>,
    pub capabilities: Vec<Capability>,
    pub health: HealthStatus,
    pub metadata: Metadata,
}
```

---

# 70. Router

Interface:

```rust
pub trait Router {
    async fn route(
        &self,
        request: &McpRequest,
        context: &RequestContext,
    ) -> Result<RouteDecision>;
}
```

Result:

```rust
pub struct RouteDecision {
    pub service: ServiceId,
    pub endpoint: Endpoint,
    pub policy: PolicyDecision,
}
```

---

# 71. Load Balancer

Interface:

```rust
pub trait LoadBalancer {
    fn select(
        &self,
        endpoints: &[Endpoint],
        context: &RequestContext,
    ) -> Option<Endpoint>;
}
```

Implementations:

```text
RoundRobin

Random

LeastConnections

LeastLatency

Weighted

Adaptive
```

---

# 72. Policy Engine

```rust
pub trait PolicyEngine {
    async fn evaluate(
        &self,
        principal: &Principal,
        request: &McpRequest,
    ) -> PolicyDecision;
}
```

Result:

```rust
pub enum PolicyDecision {
    Allow,
    Deny,
    RequireApproval,
}
```

---

# 73. Health Manager

```rust
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Draining,
    Disabled,
}
```

---

# 74. Circuit Breaker

```rust
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}
```

State should be tracked independently for each upstream endpoint.

---

# 75. Configuration

MCPMesh should support:

```text
YAML

environment variables

CLI flags

remote configuration
```

Priority:

```text
CLI

↓

Environment

↓

YAML

↓

Defaults
```

---

# 76. Database

PostgreSQL can store Control Plane state.

Tables:

```text
organizations

users

principals

servers

server_endpoints

tools

resources

prompts

routes

policies

credentials

approvals

audit_events

tasks

namespaces

clusters

configuration_versions
```

---

# 77. Redis

Redis should be optional.

Useful for:

```text
distributed rate limiting

short-lived cache

cluster coordination

temporary health state

approval state
```

MCPMesh should still support simple single-node operation without Redis.

---

# 78. NATS

NATS can be introduced for larger clusters.

Events:

```text
server.registered

server.unhealthy

server.recovered

tool.changed

policy.updated

route.updated

approval.requested

approval.completed
```

---

# 79. Caching

MCPMesh should understand MCP-aware caching.

Possible cached data:

```text
tools/list

resources/list

prompts/list

resource reads

server discovery

capabilities
```

Never blindly cache:

```text
tools/call
```

unless explicit semantics permit it.

---

# 80. Cache Architecture

```text
Agent
  │
  ▼
MCPMesh
  │
  ├── cache hit
  │       │
  │       └── response
  │
  └── cache miss
          │
          ▼
      MCP Server
```

---

# 81. Tool Catalog Cache

The global catalog should avoid calling every MCP server every time an agent asks for available tools.

```text
MCP Servers
     │
     ▼
Tool Discovery
     │
     ▼
MCPMesh Catalog
     │
     ▼
Agents
```

---

# 82. Protocol Compatibility

MCPMesh should maintain a compatibility layer.

Internally:

```text
Client Protocol

2025.x
2026.x

       ↓

Protocol Adapter

       ↓

Upstream Protocol
```

This enables gradual migration.

---

# 83. Protocol Negotiation

MCPMesh should detect:

```text
protocol version

supported extensions

server capabilities
```

and maintain this information in the registry.

---

# 84. MCP Extensions

Architecture must support extensions without modifying the core.

Example:

```text
extensions/

├── tasks
├── apps
├── enterprise-auth
└── custom
```

Interface:

```rust
pub trait McpExtension {
    fn name(&self) -> &str;

    fn register(
        &self,
        registry: &mut ExtensionRegistry
    );
}
```

---

# 85. Multi-Tenancy

MCPMesh should support organizations.

```text
Organization A

├── Agents
├── Servers
├── Tools
├── Policies
└── Secrets

Organization B

├── Agents
├── Servers
├── Tools
├── Policies
└── Secrets
```

Strict isolation is required.

---

# 86. Namespaces

Within organizations:

```text
production

development

research

support
```

Example:

```bash
mcpmesh namespace use production
```

---

# 87. High Availability

Control Plane:

```text
Control Plane 1

Control Plane 2

Control Plane 3
```

behind:

```text
Load Balancer
```

Data Plane:

```text
Gateway 1

Gateway 2

Gateway 3
```

Because modern MCP HTTP traffic can be stateless, requests can be distributed across gateway replicas.

---

# 88. Horizontal Scaling

```text
                   Load Balancer

             ┌──────────┼──────────┐
             ▼          ▼          ▼

          MCPMesh     MCPMesh    MCPMesh

          Gateway     Gateway    Gateway
```

Each gateway should avoid maintaining unnecessary local session state.

---

# 89. Kubernetes Deployment

Possible deployment:

```text
Namespace: mcpmesh-system

control-plane

gateway

dashboard

postgres

redis
```

Helm:

```bash
helm install mcpmesh mcpmesh/mcpmesh
```

---

# 90. Docker Deployment

Small installations:

```bash
docker compose up -d
```

Services:

```text
mcpmesh

postgres

dashboard
```

Redis optional.

---

# 91. Single Binary Mode

For development:

```bash
mcpmesh server
```

Could start:

```text
Control Plane

Data Plane

SQLite

Dashboard API
```

from one binary.

This makes local experimentation extremely easy.

---

# 92. Edge Mode

MCPMesh gateways could run close to agents.

```text
Central Control Plane

         │

 ┌───────┼────────┐
 ▼       ▼        ▼

US      EU      LATAM

Gateway Gateway Gateway
```

Configuration is distributed from the central control plane.

---

# 93. Zero-Downtime Configuration

Routing and policy updates should not restart gateways.

```text
Control Plane

policy update
     │
     ▼
Gateway receives config
     │
     ▼
atomic configuration swap
```

Existing requests continue normally.

---

# 94. Configuration Versioning

Every configuration change should produce:

```text
revision
```

Example:

```text
revision 821

author:
daniel

change:
github routing

timestamp:
...
```

Rollback:

```bash
mcpmesh config rollback 820
```

---

# 95. GitOps

Configuration should be compatible with Git.

Example repository:

```text
mesh/

├── servers/
│   ├── github.yaml
│   └── postgres.yaml
│
├── routes/
│   └── routes.yaml
│
└── policies/
    └── security.yaml
```

CI:

```text
git push

↓

validate

↓

security checks

↓

mcpmesh apply
```

---

# 96. Configuration Validation

Before applying:

```bash
mcpmesh validate mesh.yaml
```

Checks:

```text
schema

duplicate routes

unknown servers

policy conflicts

invalid tools

invalid timeouts

invalid weights
```

---

# 97. Dry Run

```bash
mcpmesh apply \
  -f production.yaml \
  --dry-run
```

Output:

```text
+ add server github-03

~ update github route

- remove github-01

WARNING:
policy production-write changed
```

---

# 98. Explain Routing

Extremely useful feature:

```bash
mcpmesh explain route \
  --agent coder-82 \
  --tool github.search
```

Output:

```text
Request:

agent:
coder-82

tool:
github.search


1. Authentication

PASS


2. Policy

developer-policy

ALLOW


3. Route

github-production


4. Healthy endpoints

github-01
github-02


5. Load balancing

least-latency


6. Selected

github-02


Reason:

lowest EWMA latency
```

This makes routing understandable.

---

# 99. Explain Policy

```bash
mcpmesh explain policy \
  --agent coder-82 \
  --tool database.drop
```

Output:

```text
DENY

Matched policy:

production-database-policy

Rule:

database.drop

Effect:

deny
```

---

# 100. Failure Handling

Possible failures:

```text
DNS failure

connection failure

timeout

invalid MCP response

protocol mismatch

authentication failure

authorization failure

rate limit

server overload

schema mismatch

circuit open
```

Each must have explicit behavior.

---

# 101. Graceful Degradation

If:

```text
github-mcp-01 fails
```

traffic moves to:

```text
github-mcp-02
```

If all fail:

```text
SERVICE_UNAVAILABLE
```

with structured error information.

---

# 102. Backpressure

MCPMesh should protect upstream servers.

Example:

```text
10,000 incoming requests

server capacity:
1,000
```

MCPMesh can:

```text
queue

rate limit

shed load

reject low-priority requests
```

instead of crashing the server.

---

# 103. Priority

Future version:

```yaml
priority:

  production-agent:
    weight: 100

  development-agent:
    weight: 20
```

During overload:

```text
production traffic
```

receives preference.

---

# 104. Adaptive Load Balancing

Eventually routing can consider:

```text
latency

errors

active requests

CPU

memory

historical performance
```

Score:

```text
endpoint score

=

latency
+
error penalty
+
load penalty
```

The healthiest endpoint wins.

---

# 105. MCPMesh API

Control API:

```text
GET    /api/v1/servers

POST   /api/v1/servers

GET    /api/v1/servers/:id

DELETE /api/v1/servers/:id


GET    /api/v1/tools

GET    /api/v1/routes

POST   /api/v1/routes

GET    /api/v1/policies

POST   /api/v1/policies

GET    /api/v1/tasks

GET    /api/v1/audit

GET    /api/v1/metrics
```

---

# 106. WebSocket / SSE API

Dashboard updates:

```text
server.health.changed

traffic.event

policy.denied

approval.requested

tool.changed
```

---

# 107. SDK

TypeScript:

```bash
npm install @mcpmesh/sdk
```

Example:

```typescript
import { MCPMesh } from "@mcpmesh/sdk";

const mesh = new MCPMesh({
  endpoint: process.env.MCPMESH_URL
});

const servers = await mesh.servers.list();
```

Python:

```bash
pip install mcpmesh
```

---

# 108. Integration With Existing Agents

Agent frameworks should see MCPMesh as another MCP endpoint.

Conceptually:

```text
OpenAI Agent
     │
     ▼
MCPMesh
     │
     ▼
All organization MCP tools
```

Same for other compatible clients.

The agent framework does not need to understand MCPMesh's internal architecture.

---

# 109. Global MCP Endpoint

One interesting mode:

```text
https://mesh.company.com/mcp
```

The organization exposes one logical MCP endpoint.

Behind it:

```text
GitHub

Slack

Jira

PostgreSQL

Internal APIs

Cloud infrastructure
```

MCPMesh builds a unified catalog.

---

# 110. Virtual MCP Servers

A particularly powerful feature.

MCPMesh can expose virtual servers.

Example:

```yaml
kind: VirtualMCPServer

metadata:
  name: developer-tools

spec:

  include:

    - github.*

    - jira.*

    - postgres.read

  exclude:

    - github.repository.delete

    - postgres.write
```

Agent connects to:

```text
/mcp/developer-tools
```

and only sees approved capabilities.

---

# 111. Role-Based Virtual Servers

Examples:

```text
/mcp/developer

/mcp/support

/mcp/security

/mcp/data

/mcp/finance
```

Each exposes a different tool catalog.

This reduces both security exposure and tool-selection noise.

---

# 112. Dynamic Tool Filtering

Two agents connected to the same MCPMesh endpoint may see different tools.

```text
Developer Agent

sees:

github
jira
postgres-read
```

while:

```text
Finance Agent

sees:

stripe
accounting
reports
```

Tool visibility itself becomes a policy decision.

---

# 113. Tool Metadata Enrichment

MCPMesh can attach operational metadata.

Example:

```json
{
  "tool": "database.execute",
  "mesh": {
    "risk": "high",
    "owner": "platform-team",
    "environment": "production",
    "approvalRequired": true
  }
}
```

---

# 114. Tool Versioning

MCPMesh can maintain:

```text
github.create_pr@1

github.create_pr@2
```

Routes allow gradual migrations.

---

# 115. Tool Deprecation

State:

```text
ACTIVE

DEPRECATED

DISABLED
```

CLI:

```bash
mcpmesh deprecate tool github.old_search
```

Agents can receive metadata indicating replacement tools.

---

# 116. Development Mode

Local developer experience:

```bash
mcpmesh dev
```

Starts local gateway.

Then:

```bash
mcpmesh add \
  github \
  http://localhost:3001/mcp
```

and:

```bash
mcpmesh add \
  database \
  http://localhost:3002/mcp
```

Agent connects only to:

```text
localhost MCPMesh
```

---

# 117. Local stdio Bridge

Many MCP servers use stdio.

MCPMesh should support:

```text
remote HTTP
        │
        ▼
MCPMesh
        │
        ▼
stdio process
```

Example configuration:

```yaml
transport:

  type: stdio

  command: npx

  args:
    - my-mcp-server
```

MCPMesh manages the process lifecycle.

---

# 118. Process Supervisor

For stdio MCP servers:

```text
STARTING

RUNNING

FAILED

RESTARTING

STOPPED
```

MCPMesh can automatically restart failed local servers.

---

# 119. Sandbox for Local MCP Servers

Future security feature:

```text
MCP server

↓

sandbox

↓

restricted filesystem

restricted network

restricted environment
```

Potential technologies:

```text
Docker

gVisor

Firecracker

WASM
```

---

# 120. Security Model

Trust boundaries:

```text
User

↓

Agent

↓

MCPMesh

↓

MCP Server

↓

External API
```

Each boundary requires explicit identity and policy decisions.

Never assume:

```text
authenticated = authorized
```

or:

```text
known server = trusted forever
```

---

# 121. Threat Model

MCPMesh should explicitly defend against:

```text
stolen credentials

token passthrough

confused deputy

SSRF

malicious MCP servers

tool definition mutation

tool name collision

unauthorized tool access

denial of service

excessive requests

credential leakage

misrouting

cross-tenant access

malformed MCP messages

schema bombs

oversized payloads

slow upstream attacks
```

---

# 122. Payload Protection

Configuration:

```yaml
limits:

  requestBody: 10MB

  responseBody: 50MB

  schemaDepth: 32

  toolCount: 5000
```

Protect against resource exhaustion.

---

# 123. Schema Validation

MCPMesh should validate schemas with bounded:

```text
depth

size

complexity

processing time
```

External schema references should not be automatically dereferenced unless explicitly configured.

---

# 124. Certificates

Internal deployments should support mTLS.

```text
Agent
   │ mTLS
   ▼
MCPMesh
   │ mTLS
   ▼
MCP Server
```

Certificate rotation should be automated.

---

# 125. Server Identity

Each registered MCP server should have a stable identity.

```text
spiffe://company/mcp/github
```

SPIFFE/SPIRE integration could be added later.

---

# 126. Multi-Cluster

Large organizations:

```text
                    Global Control Plane

                    /        |        \

                   /         |         \

                  ▼          ▼          ▼

               Cluster     Cluster    Cluster

                  US          EU       LATAM
```

Each cluster has local gateways.

---

# 127. Federation

Different MCPMesh installations could federate.

```text
Company A Mesh

       │

       │ trusted federation

       ▼

Company B Mesh
```

Policies determine which tools can cross boundaries.

This should be a later feature.

---

# 128. MCPMesh Plugin System

Extensions:

```text
Authentication plugins

Discovery plugins

Policy plugins

Telemetry plugins

Secret providers

Load balancers

Filters
```

Rust interface:

```rust
pub trait MeshPlugin {
    fn name(&self) -> &'static str;

    async fn initialize(
        &self,
        context: PluginContext
    ) -> Result<()>;
}
```

---

# 129. WebAssembly Plugins

Eventually plugins could run as WASM.

Advantages:

```text
language independence

sandboxing

hot loading

security

portable extensions
```

Architecture:

```text
MCPMesh

  ↓

WASM Plugin Runtime

  ├── custom policy

  ├── custom routing

  └── custom auth
```

---

# 130. Performance Goals

Initial targets should be benchmarked rather than advertised as guarantees.

Measure:

```text
requests/sec

P50 latency

P95 latency

P99 latency

memory/request

CPU/request

connection count

tool catalog size

number of upstream servers
```

Benchmarks should compare:

```text
direct MCP

vs

MCP through MCPMesh
```

The key metric:

```text
proxy overhead
```

---

# 131. Benchmark Suite

Repository:

```text
benchmarks/

├── routing
├── policy
├── proxy
├── load-balancing
├── tool-catalog
└── rate-limit
```

CI should detect performance regressions.

---

# 132. MVP

The first version should NOT implement everything in this document.

MVP architecture:

```text
MCP Client

    ↓

MCPMesh

    ├── Registry
    ├── Routing
    ├── Load Balancing
    ├── Health Checks
    ├── Circuit Breaker
    ├── Basic Policies
    ├── Rate Limiting
    └── OpenTelemetry

    ↓

MCP Servers
```

---

# 133. MVP Features

Version `0.1`:

```text
Streamable HTTP proxy

stdio bridge

MCP server registry

tool discovery

tool catalog

header-based routing

round-robin

least-connections

health checking

circuit breaker

timeouts

basic retries

JWT authentication

API keys

basic RBAC

rate limiting

structured logs

Prometheus metrics

OpenTelemetry traces

CLI

YAML configuration
```

---

# 134. MVP Demo

Start three MCP servers:

```text
github-mcp-01

github-mcp-02

postgres-mcp-01
```

Register:

```bash
mcpmesh add github \
  https://github-01/mcp

mcpmesh add github \
  https://github-02/mcp

mcpmesh add postgres \
  https://postgres-01/mcp
```

Agent connects to:

```text
http://localhost:8080/mcp
```

Agent requests:

```text
github.search
```

MCPMesh:

```text
authenticate

↓

authorize

↓

resolve github

↓

find healthy replicas

↓

least connections

↓

github-mcp-02

↓

trace

↓

response
```

---

# 135. Failure Demo

While requests are running:

```text
kill github-mcp-02
```

MCPMesh detects:

```text
connection failure

↓

health degraded

↓

circuit open

↓

remove endpoint
```

Traffic moves:

```text
github-mcp-01
```

without changing agent configuration.

This would be one of the first public demos.

---

# 136. Security Demo

Agent:

```text
database.drop
```

MCPMesh:

```text
Identity:

coding-agent


Policy:

developer


Rule:

database.drop = DENY


Result:

BLOCKED
```

Dashboard immediately displays the security event.

---

# 137. Rate Limit Demo

Agent sends:

```text
1,000 requests/sec
```

Policy:

```text
100 requests/sec
```

MCPMesh:

```text
first 100

ALLOW

remaining

RATE_LIMITED
```

while other agents remain unaffected.

---

# 138. Load Balancing Demo

Three replicas:

```text
github-01

latency 28ms

github-02

latency 80ms

github-03

latency 210ms
```

Adaptive routing gradually favors:

```text
github-01
```

without completely starving other healthy replicas.

---

# 139. Roadmap

## Phase 1 — Protocol

Implement:

```text
MCP proxy

Streamable HTTP

stdio bridge

protocol parsing

version compatibility
```

---

## Phase 2 — Registry

Implement:

```text
server registry

tool discovery

tool catalog

capabilities

health state
```

---

## Phase 3 — Routing

Implement:

```text
routes

round robin

least connections

weighted routing

timeouts

retries
```

---

## Phase 4 — Reliability

Implement:

```text
health checks

circuit breakers

backpressure

graceful draining

failure recovery
```

---

## Phase 5 — Security

Implement:

```text
authentication

authorization

RBAC

policies

rate limiting

audit logs

credential isolation
```

---

## Phase 6 — Observability

Implement:

```text
Prometheus

OpenTelemetry

structured logging

distributed tracing

traffic inspection
```

---

## Phase 7 — Developer Experience

Implement:

```text
CLI

dashboard

Docker

Helm

GitOps

explain routing

explain policy
```

---

## Phase 8 — Advanced Routing

Implement:

```text
adaptive routing

regional routing

canary

traffic splitting

tool versioning

virtual MCP servers
```

---

## Phase 9 — Enterprise

Implement:

```text
multi-tenancy

SSO

OIDC

mTLS

external secret providers

SIEM

immutable auditing

multi-cluster
```

---

## Phase 10 — Ecosystem

Implement:

```text
plugins

WASM filters

SDKs

service discovery adapters

federation
```

---

# 140. What NOT to Build First

Do not initially build:

```text
AI-based routing

MCP marketplace

billing

federation

complex dashboard

multi-cloud

custom secret manager

custom identity provider

custom database

custom protocol
```

Use existing standards.

Focus on:

```text
Proxy

↓

Discovery

↓

Routing

↓

Reliability

↓

Security

↓

Observability
```

---

# 141. First Repository Structure

Start much smaller than the final architecture.

```text
mcpmesh/

├── crates/
│
│   ├── core/
│   ├── proxy/
│   ├── registry/
│   ├── router/
│   ├── policy/
│   ├── telemetry/
│   └── cli/
│
├── examples/
│
├── config/
│
├── tests/
│
├── docs/
│
├── Cargo.toml
│
└── README.md
```

---

# 142. First Development Milestone

The first milestone should prove this:

```text
MCP Client

     ↓

MCPMesh

     ↓

2 replicated MCP servers
```

Required functionality:

```text
1. Receive MCP request.

2. Read MCP method/name.

3. Find destination service.

4. Select healthy endpoint.

5. Forward request.

6. Return MCP response.

7. Emit trace.

8. Emit metrics.

9. Detect failed endpoint.

10. Route next request elsewhere.
```

Nothing more is necessary to prove the architecture.

---

# 143. Second Milestone

Add:

```text
Tool discovery

Tool registry

RBAC

Rate limiting

Circuit breakers
```

Then demonstrate:

```text
Agent A

CAN:
github.read

CANNOT:
github.delete


Agent B

CAN:
github.read
github.write
```

---

# 144. Third Milestone

Introduce:

```text
Virtual MCP Servers
```

Example:

```text
/mcp/developer
```

exposes:

```text
github

jira

postgres-read
```

while:

```text
/mcp/devops
```

exposes:

```text
github

kubernetes

aws

grafana
```

This could become one of MCPMesh's strongest features.

---

# 145. Fourth Milestone

Introduce:

```text
MCPMesh Dashboard
```

Show:

```text
servers

tools

topology

traffic

latency

errors

security

policies

traces
```

---

# 146. Future: Intelligent Routing

Once enough telemetry exists, MCPMesh could eventually route based on historical performance.

Example:

```text
Tool:

search
```

Available servers:

```text
Server A

P95:
80ms

success:
99.9%


Server B

P95:
210ms

success:
99.99%


Server C

P95:
45ms

success:
97%
```

MCPMesh can calculate a dynamic score.

But this should come after deterministic routing works reliably.

---

# 147. Future: AI-Aware Routing

Eventually routing can consider agent context.

Example:

```text
research agent

↓

search capability

↓

MCPMesh
```

Available:

```text
web-search-mcp

internal-search-mcp

github-search-mcp
```

Routing could consider:

```text
agent role

data classification

organization

task type

latency

cost

policy
```

This is a future feature, not MVP behavior.

---

# 148. Future: MCPMesh + AgentFirewall

MCPMesh could later provide or integrate a deeper security layer.

```text
Agent

↓

MCPMesh

↓

AgentFirewall

↓

MCP Server
```

or AgentFirewall could become a native MCPMesh module:

```text
MCPMesh

├── Router
├── Registry
├── Firewall
├── Identity
├── Policy
└── Telemetry
```

---

# 149. Future: MCPMesh + AgentVault

Credentials:

```text
Agent

↓

MCPMesh

↓

Credential Broker

↓

MCP Server

↓

External API
```

The agent never receives the external API credential.

This could eventually become:

```text
MCPMesh Vault
```

or remain an external integration.

---

# 150. Long-Term Architecture

The complete platform could eventually look like:

```text
                           AI APPLICATIONS

                  ┌────────────┼────────────┐
                  ▼            ▼            ▼

               OpenAI       Claude       Custom
                Agent        Agent         Agent

                    \          |          /
                     \         |         /
                      ▼        ▼        ▼

                    ┌─────────────────────┐
                    │                     │
                    │       MCPMesh       │
                    │                     │
                    ├─────────────────────┤
                    │ Service Discovery   │
                    │ Tool Registry       │
                    │ Routing             │
                    │ Load Balancing      │
                    │ Authentication      │
                    │ Authorization       │
                    │ Policies            │
                    │ Rate Limiting       │
                    │ Circuit Breakers    │
                    │ Retries             │
                    │ Caching             │
                    │ Credentials         │
                    │ Auditing            │
                    │ Observability       │
                    └──────────┬──────────┘
                               │

           ┌───────────────────┼─────────────────────┐
           │                   │                     │
           ▼                   ▼                     ▼

      Engineering            Business             Infrastructure

           │                   │                     │

      ┌────┼────┐         ┌────┼────┐          ┌─────┼─────┐
      ▼    ▼    ▼         ▼    ▼    ▼          ▼     ▼     ▼

   GitHub Jira DB       Slack CRM Stripe       AWS   K8s Grafana

    MCP   MCP MCP        MCP  MCP  MCP         MCP   MCP   MCP
```

---

# 151. Product Identity

## Name

**MCPMesh**

## Tagline

> **The Service Mesh for AI Tools.**

## Alternative tagline

> **Secure, route and observe MCP at scale.**

## Technical description

> MCPMesh is an open-source service mesh and gateway for Model Context Protocol infrastructure. It provides service discovery, intelligent routing, load balancing, authentication, authorization, policy enforcement, rate limiting, health checking, circuit breaking, auditing and distributed observability for MCP servers.

---

# 152. GitHub Description

Short:

> High-performance service mesh for MCP — discovery, routing, security, load balancing and observability for AI tools.

Longer:

> MCPMesh is an open-source infrastructure layer for running Model Context Protocol at scale. Discover, route, secure, balance, monitor and govern MCP servers through a high-performance Rust data plane.

---

# 153. README Hero

```text
MCPMesh
=======

The Service Mesh for AI Tools.

Secure, route and observe MCP at scale.

✓ Service Discovery
✓ Tool Registry
✓ Intelligent Routing
✓ Load Balancing
✓ Health Checking
✓ Circuit Breakers
✓ Authentication
✓ Authorization
✓ Rate Limiting
✓ Policy Enforcement
✓ Distributed Tracing
✓ Auditing
✓ Multi-Tenant
✓ Open Source
✓ Built in Rust
```

---

# 154. The Core Experience

The eventual user experience should be extremely simple.

Install:

```bash
cargo install mcpmesh
```

Start:

```bash
mcpmesh init
```

Register:

```bash
mcpmesh add github \
  https://github.internal/mcp
```

Register another replica:

```bash
mcpmesh add github \
  https://github-02.internal/mcp
```

Start gateway:

```bash
mcpmesh serve
```

Inspect:

```bash
mcpmesh get servers
```

Output:

```text
NAME       REPLICAS    STATUS

github     2           Healthy
```

Agent connects to:

```text
MCPMesh
```

instead of managing individual MCP servers.

From that point MCPMesh handles:

```text
Discovery

Routing

Load Balancing

Failures

Security

Policies

Rate Limits

Tracing

Metrics

Auditing
```

---

# 155. Final Vision

Today:

```text
AI Agent

   │

   ├── MCP Server
   ├── MCP Server
   ├── MCP Server
   ├── MCP Server
   └── MCP Server
```

Tomorrow:

```text
                     AI Agents

                         │

                         ▼

                     MCPMesh

                         │

              ┌──────────┼──────────┐

              ▼          ▼          ▼

           MCP Fleet   MCP Fleet   MCP Fleet

              │          │          │

           Tools      Services     Data
```

The long-term goal is for developers to stop thinking about individual MCP endpoints.

They should think in terms of:

```text
capabilities

services

policies

identities

routes
```

MCPMesh determines:

```text
WHERE a tool lives

WHICH server should receive a request

WHETHER the caller can use it

HOW traffic should be balanced

WHAT happens when a server fails

HOW credentials are handled

HOW requests are limited

HOW calls are traced

HOW actions are audited

HOW MCP infrastructure scales
```

The fundamental proposition is:

> **MCP standardizes how AI communicates with tools. MCPMesh standardizes how those tools are operated at scale.**
