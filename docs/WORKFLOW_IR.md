# Workflow IR

The Workflow IR is the portable contract at the center of AgentMesh Teach.
Recorders produce it (via semantic traces), the validator accepts or rejects
it, runtimes execute it, and compilers turn it into MCP tools, CLIs, or REST
endpoints.

The IR describes _what_ a learned capability does — never _how_ it was
captured. Coordinates, screenshots, and raw events belong in recordings; the
IR only keeps semantic targets plus `{{ inputs.* }}` and `{{ steps.* }}`
templates.

## Example

```yaml
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
  denied_operations:
    - file.*
  max_runs_per_hour: 60
preconditions:
  - op: assert.exists
    target: { semantic: conversation_search, role: textbox }
success:
  - op: assert.exists
    target: { semantic: message_list }
failure:
  - op: assert.text
    value: Session expired
recovery:
  max_attempts: 2
  checkpoints: [open_whatsapp]
  capture_aria: true
  capture_screenshot: true
observed_apis:
  - method: GET
    host: web.whatsapp.com
    path: /api/messages
    query_keys: [limit]
```

## Schema rules

- `version` must be `"1.0"`.
- `id` is dot-separated lowercase segments (`app.capability`); it doubles as
  the store file name (`<id>.yaml` under `~/.agentmesh/workflows/`, or
  `$AGENTMESH_HOME/workflows/`).
- `runtime` is one of `browser`, `desktop`, `mobile`, `api` (preferred
  runtime; the router may fall back).
- `inputs` map names to `{ type, required, default }`. Types: `string`,
  `integer`, `number`, `boolean`, `array`, `object`, `datetime`. Unset
  `required` means required, unless a `default` is present (a default implies
  optional). Explicit `required: true` with a default is rejected, and
  defaults must match their declared type.
- `steps` is a non-empty ordered list with unique lowercase `id`s. `op` must
  come from the operation families below. `browser.navigate` needs a `url`;
  most `ui.*` ops (except `ui.wait`) need a `target`. Inference emits only
  the entry navigation as `browser.navigate`; every navigation observed
  mid-flow is an effect of an earlier interaction (a submit redirecting, an
  SPA route change), so it becomes `assert.url` with the observed URL — the
  runtime waits for that URL instead of loading it again. Recorded URLs often
  carry volatile query strings (tracking, highlight state), so `workflows
  relax` generalizes `assert.url` to scheme/host/path and drops duplicate
  assertions; the previous revision is snapshotted before overwriting.
- Templates use `{{ inputs.<name> }}` (must be declared) and
  `{{ steps.<id>[.result] }}` (the step must exist and precede the reference).
  Step results resolve at runtime too, so later `value`, `url`, `limit`,
  `condition`, `iterations`, and `path` templates can consume earlier outputs.
- `control.if` / `control.switch` need a `condition` template: when it renders
  falsy (empty, `false`, `0`, `no`, `null`, ...) the next step is skipped.
  `control.loop` needs an `iterations` template (1–100): the next step repeats
  that many times. `control.retry` / `control.timeout` stay declarative
  markers owned by the recovery policy.
- `ui.drag` / `ui.drop` take an optional `destination` target, or an `"x,y"`
  pixel offset in `value`. `file.*` steps need a `path` (or `value`) template:
  `file.choose`/`file.upload` attach a file, `file.download` clicks then saves
  the download to `path`, `file.save` writes redacted text to `path`.
- `outputs` bind names to `steps.<id>[.result]` of a declared step.
- `policy` (optional): `allowed_origins` must be `http(s)` URLs;
  `allowed_operations` and `denied_operations` accept exact operations or
  family wildcards such as `ui.*` and `file.*` (denials take precedence);
  `max_runs_per_hour` must be above zero when set.
- `preconditions`, `success`, and `failure` are state predicates using
  `assert.exists`, `assert.not_exists`, `assert.text`, or `assert.url`.
  Preconditions run after entry navigation, failure detectors run after each
  step, and success verifiers run before outputs are returned.
- `recovery` controls bounded retries (1–5), URL checkpoints, ARIA observation,
  failure screenshots, and an optional external visual-repair adapter. Visual
  proposals are artifacts for review; they are never auto-applied.
- `observed_apis` contains sanitized request shapes learned while recording:
  method, host, path, and query-key names only. Bodies, query values, headers,
  cookies, and credentials are never stored. Observations are non-executable
  hints for reviewed API-adapter generation.

## Operation families

| Family | Ops |
| --- | --- |
| Browser | `browser.navigate`, `browser.back`, `browser.forward`, `browser.new_tab`, `browser.close_tab`, `browser.wait_navigation` |
| UI | `ui.find`, `ui.focus`, `ui.fill`, `ui.click`, `ui.activate`, `ui.press`, `ui.select`, `ui.scroll`, `ui.drag`, `ui.drop`, `ui.extract`, `ui.wait` |
| Application | `app.open`, `app.close`, `app.focus` |
| Files | `file.choose`, `file.upload`, `file.download`, `file.save` |
| Authentication | `auth.ensure_session`, `auth.request_secret`, `auth.require_user` |
| Control flow | `control.if`, `control.switch`, `control.loop`, `control.retry`, `control.timeout` |
| Assertions | `assert.exists`, `assert.not_exists`, `assert.text`, `assert.url`, `assert.schema`, `assert.state` |
| Human interaction | `human.confirm`, `human.authenticate`, `human.resolve` |

Unknown ops fail closed: a workflow can never reference behavior no runtime
implements. The browser executor runs every documented op: tabs
(`browser.new_tab`, `browser.close_tab`, `browser.wait_navigation`), element
interaction (`ui.focus`, `ui.scroll`, `ui.drag`, `ui.drop`), app-focus helpers
(`app.*` map to tabs in the browser runtime), file operations (`file.*`),
session/secret gates (`auth.ensure_session`, `auth.request_secret`,
`auth.require_user`), structured control flow (`control.if`/`switch`/`loop`),
and state checks (`assert.schema` validates the latest extracted result
against a type or `{"type","minItems"}` descriptor; `assert.state` accepts a
`url:` prefix or matches page text/URL). Desktop, mobile, and API workflows
use explicit executables configured as `AGENTMESH_RUNTIME_<RUNTIME>`; visual
repair uses `AGENTMESH_VISION_ADAPTER[_<NAME>]`.

## Targets

A target carries at least one resolution signal; coordinates are deliberately
absent:

```yaml
target:
  semantic: search_conversations
  role: button
  accessible_name: Search
  text: Go
  placeholder: Teléfono
  autocomplete: username
  selectors:
    - "[data-testid=search]"
  match: "{{ inputs.contact }}"
```

The resolver order is: validated selector, role with accessible name,
visible text (substring), accessible name alone, input placeholder,
`autocomplete` attribute, DOM semantics, and finally a bare role with no
name (reported at low confidence). The `autocomplete` signal (`username`,
`current-password`, ...) stays stable on login forms whose ids rotate on
every page load. Recorded visible text and placeholders are kept truncated as
fallbacks for elements without names; steps relying on positional selectors
alone are flagged `[!]` at review time. A bare role never executes blindly:
when recorded text or a name matched nothing, the run fails closed with a
`target.blind_match_refused` audit event instead of clicking the first match.

## Key presses and secrets

`ui.press` replays a keyboard press on a target (`value` is the key,
defaulting to `Enter`). The recorder emits a form submission whenever Enter
is pressed inside a field, so login flows that submit with Enter replay the
key instead of clicking the field (which would do nothing).

Password fields never record values: each becomes a `secret://` reference.
At review time Teach asks for a secret name per reference (`linkedin_password`
instead of an unstable page id), collapses repeated secret fills on the same
field, and offers to store the value in `$AGENTMESH_HOME/.env` (mode 0600,
never in the workflow). Runs load that file without overriding real
environment variables; a missing secret fails closed (`secret unavailable:
<ref> (set <ENV_VAR>)`). `agentmesh secret list` shows every reference with
its variable and whether it is set.

## CLI

```bash
agentmesh teach --name whatsapp.read_messages   # guided authoring
agentmesh workflows list                        # stored workflows
agentmesh workflows inspect whatsapp.read_messages
agentmesh workflows validate ./workflow.yaml    # file or stored id
```

Every load re-validates: the store never serves a workflow the validator
would reject, and a corrupt document fails `list` loudly instead of hiding.

## Operational intelligence

Teach does not silently rewrite model weights. It gives a model durable,
retrievable operational intelligence: validated workflows, repeated semantic
routines, typed MCP contracts, execution experience, observations, and
redacted trajectory datasets. A separate training system can consume
`agentmesh workflows dataset --out trajectories.jsonl` when actual fine-tuning
is desired.
