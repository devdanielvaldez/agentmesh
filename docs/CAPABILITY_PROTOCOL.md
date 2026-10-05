# AgentMesh Capability Protocol (AMCP)

AgentMesh Capability Protocol is an open, portable contract for an outcome an
agent can achieve. It does not replace MCP, A2A, OpenAPI/Arazzo, Skills, or
AgentMesh Teach workflows. Instead, it defines the layer above them: an
outcome-oriented contract with explicit authority, effects, verification,
recovery, provenance, and alternative execution bindings.

## Design boundary

```text
Capability package: what may be achieved and under which guarantees
    └── implementation binding: how one runtime achieves it
            ├── MCP
            ├── A2A
            ├── OpenAPI/Arazzo
            ├── AgentMesh Workflow IR / Teach
            ├── CLI
            └── human-assisted execution
```

The current Rust reference model is in `agentmesh-protocol`. A package is
valid only when it has a namespaced identity, input/output schemas, at least
one success-evidence claim, unambiguous authority requirements, and at least
one implementation binding.

## Minimal example

```yaml
schemaVersion: amcp/0.1
capability:
  id: billing.refund
  version: 1.0.0
  intent: Refund an eligible customer payment
contract:
  inputs: { type: object, required: [payment_id] }
  outputs: { type: object, required: [refund_id] }
  successEvidence:
    - id: payment_refunded
      assertion: payment.status == refunded
      acceptedTypes: [provider_receipt, state_assertion]
  effects: [financial, external_communication]
  idempotency: key_required
  recovery: reconcile
authority:
  permissions: [customer.read, payment.refund]
  approvals:
    - before: execute_refund
      display: [customer, amount]
      requiredRole: finance_approver
implementations:
  - id: stripe-api
    kind: mcp
    reference: mcp://stripe/refund
```

`completed` means execution stopped normally. `verified` will be reserved for
a future runtime module that validates the declared evidence claims. A runtime
must never equate those states merely because the final tool returned HTTP 200.

## Teach lifecycle integration

When `agentmesh teach` or guided workflow authoring saves a workflow with one or
more `success` assertions, `agentmesh-teach` compiles it to a package under
`<AgentMesh data home>/capabilities/learned/<capability-id>/<workflow-ir-version>.yaml`.
The package is immediately visible to the local capability catalog and the
interactive console. A workflow without success assertions remains executable
as a workflow but is deliberately not compiled as an AMCP package. Re-saving
updates the live learned package while preserving its prior package document
under `capability-revisions/`; deleting or renaming the workflow removes its
generated package from live discovery. The success assertions are contract
claims, not a certification: a later runtime execution must return matching
evidence before it can be called verified.

## Approval checkpoints

The policy crate binds each approval to the package ID and version, checkpoint,
and canonical action inputs. Only the fields named by the checkpoint are
returned for display; the approval store retains a SHA-256 fingerprint instead
of the raw values. Before execution, the runtime can recompute the fingerprint
and reject an approval if any action input changed.

## Composition

`contract.requires` declares child capabilities by namespaced ID and optional
exact version. The protocol crate resolves a dependency first execution order.
If an unconstrained requirement has multiple catalog versions, resolution asks
for an exact version instead of guessing. Missing dependencies and cycles fail
closed before a runtime starts any implementation.

The reference discovery function ranks candidates by ID and intent text. Its
score is advisory only: it does not assert equivalence, satisfy dependencies,
or grant permissions. Production deployments can add embedding or domain
search behind the same result contract while retaining those checks.

## Implementation selection

The runtime supplies installed binding IDs and an ordered preference for
binding kinds. Selection is deterministic and only returns a binding already
declared by the package and available to that runtime. Admission and request
authorization must succeed before invoking the selected implementation.

## Recovery decisions

The protocol crate provides a bounded recovery decision function. Unknown
external state always triggers reconciliation first. A retry requires remaining
attempt budget and either a guaranteed idempotent operation or the original
idempotency key. Compensation is selected only when the runtime confirms that
the binding has a tested compensating operation.

## Certification

Certification reports are bound to an exact capability ID, version, and content
digest. The evaluator advances through `draft`, `reviewed`, `tested`, `verified`,
and `production_certified` only when the required named checks have retained
evidence references. It validates submitted claims; a trusted test runner and
signature verifier must still validate those referenced artifacts.

## Package integrity

Packages can declare a `sha256:` content digest. The digest covers the package,
including publisher and source, while normalizing the digest and signature
fields to avoid circularity. Admission rejects a declared digest that does not
match the loaded package. This establishes integrity and provenance consistency;
cryptographic publisher signature verification requires a trusted key resolver
and remains a separate registry/runtime responsibility.

The policy crate now exposes a publisher trust interface that checks the
recomputed package digest, optional publisher allowlist, and a signature through
an injected `PackageSignatureVerifier`. The trust result is separate from
execution admission; a trusted package can still be denied by risk, permission,
or approval policy. Key resolution and cryptographic algorithms are supplied by
the deployment.

## Agent negotiation

An agent can request an exact capability ID and optional version while declaring
the effects it accepts, permissions it has delegated, installed bindings, and
runtime preferences. The provider returns a concrete offer with schemas,
effects, requested permissions, binding, and package digest. The offer does not
grant authority; the requester still checks publisher trust and local policy.

## Runtime execution

`agentmesh-runtime` now exposes a capability execution entry point. It validates
the input object against the declared required fields and primitive types,
applies capability admission, selects an installed binding, invokes its
executor adapter, and builds a receipt from the selected package and binding.
It marks success `verified` only when every declared evidence claim is present
with an accepted type. Approval-required and denied capabilities stop before
the executor runs.
