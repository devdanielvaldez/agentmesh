# AgentMesh Teach Manual: teach, run, and publish your first workflow

> Hands-on walkthrough for trying the Teach feature end to end.
> The technical reference lives in `docs/WORKFLOW_IR.md`.
> Estimated time: 20–30 minutes.

With Teach you turn a browser demonstration into a reusable workflow:

```
browser (CDP recording) → semantic trace → inference → workflow YAML
    → Playwright execution (with self-healing) → replay → MCP server
```

Everything you try here stays isolated thanks to `AGENTMESH_HOME`: it never
touches your real `~/.agentmesh`.

---

## 0. Requirements

| Requirement | How to check it |
|---|---|
| Rust (cargo) | `cargo --version` |
| Node.js ≥ 20 | `node --version` (tested with v22) |
| Google Chrome installed | `google-chrome --version` on Linux, or Chrome in `/Applications` on macOS. The recorder uses it via `playwright-core`, without downloading browsers |
| This repository | built once (step 1) |

## 1. Build

```bash
cd /Users/user/Documents/Projects/AgentMesh

# Rust engine
cargo build -p agentmesh

# Teach TypeScript runtime (recorder + executor + sessions)
cd teach && npm install && npm run build && cd ..

# Resulting binary
./target/debug/agentmesh --help
```

The binary finds the Teach runtime (`teach/dist`) only when you work from the
repo root. If you plan to invoke `agentmesh` from another directory, export:

```bash
export AGENTMESH_TEACH_DIST=/Users/user/Documents/Projects/AgentMesh/teach/dist
```

Isolate your test lab (recommended):

```bash
export AGENTMESH_HOME=/tmp/teach-lab
mkdir -p /tmp/teach-lab/workflows
```

## 2. Prepare the demo page

Create a tiny local app to teach searching. Save this as `/tmp/teach-e2e/page.html`:

```html
<!doctype html><html><body>
<form onsubmit="return false">
<input aria-label="Search" id="q" />
<button id="go">Go</button>
</form>
<ul id="items"><li>alpha</li><li>beta</li><li>gamma</li></ul>
<script>document.getElementById('go').addEventListener('click', () => {
  const v = document.getElementById('q').value;
  document.getElementById('items').innerHTML = '<li>found:' + v + '</li>';
});</script>
</body></html>
```

And serve it locally (leave this terminal open):

```bash
python3 -m http.server 3221 --directory /tmp/teach-e2e
```

## 3. Teach your first workflow (browser recording)

```bash
agentmesh teach --target browser \
  --name demo.search --headed \
  --start-url http://127.0.0.1:3221/page.html \
  --scope http://127.0.0.1:3221
```

What happens:

1. A **visible** Chrome opens with the page. Visible mode is the default;
   use `--headless` only for unattended automation via `--cdp-port`.
2. **Act as the user**: type something in the field (e.g. `beta`) and press **Go**.
   Every captured event appears in your terminal instantly, e.g.
   `recorded #3 ui.fill input textbox "Search" #q = "beta"`.
   - To **capture the text of an element** (a list, a result, a table):
     hold **Alt and click it**. You will see `recorded #N ui.extract ... (K items)`
     and inference will create a `ui.extract` step (e.g. `read_5`).
3. Go back to the terminal and press **Enter** to stop recording.
4. You will see what was inferred, e.g. `Inferred 4 steps and 1 parameter candidates`, and the assistant will ask:
   - `Parameter name for "beta" [search]` → type `skip` (for now we keep
     the literal value; we will parameterize it in step 7).
   - `Description (optional)` → Enter.
   - `Preferred runtime [browser]` → Enter.
   - `Output name (empty to finish)` → Enter (no outputs for now). If you recorded
     probes with Alt+click, it then offers `Expose read_5 result as output`:
     type a name (e.g. `results`) or Enter to skip it.
   - `Add a policy? [y/N]` → Enter (no policy).
   - `Save this workflow? [Y/n]` → Enter. This is your final review: first you
     see the detailed listing of every step (`Detected workflow:` with role, name,
     selector, and value). Answer `n` to discard without saving anything.
5. `Saved demo.search (4 steps) to .../workflows/demo.search.yaml`.

> Notes: the recorder never stores coordinates or secrets (password fields
> become `secret://` references). If another recorder is still alive on the
> same trace or the CDP port is busy, it refuses to start with a clear message
> instead of corrupting the trace. By default inference compacts repetitions
> (43 keystrokes → 1 step with the final value); if you want every event as
> its own literal step, record with `--raw`.

## 4. Look at what was stored

```bash
agentmesh workflows list
agentmesh workflows inspect demo.search
```

You will see the IR: `browser.navigate` only for entry; navigations that
happen mid-flow (a redirect after submit, an SPA route change) are stored as
`expect_N (assert.url)`: the MCP does not reload them, it waits for them to
happen. Then `ui.fill` with the literal value `beta`, and `ui.click`
with a semantic descriptor (role + name) plus fallback selectors. (In step 7
that literal becomes `{{ inputs.search }}`.)

## 5. Run it

```bash
agentmesh workflows run demo.search --yes
```

It should finish with `exit 0` and leave a `Run: run_...` line plus the
`audit.jsonl` path. You just executed in Playwright what you taught by hand
(with the recorded literal value `beta`). With `--headed` you watch the run in
a visible Chrome (`AGENTMESH_HEADED=1` works too); without it, it runs
headless. Every step lands in the `audit.jsonl`: which strategy resolved it
(`selector`, `role`, `text`...), healings, proposed repairs, and the exact
error on failure.

No side effects (only resolves selectors and validates):

```bash
agentmesh workflows test demo.search
```

Re-run exactly the previous run with its original inputs:

```bash
agentmesh replay <RUN_ID> --yes
```

## 6. Self-healing: break a selector and watch it recover

1. Edit `$AGENTMESH_HOME/workflows/demo.search.yaml` and change the click
   selector, e.g. `'#go'` → `'#button-that-does-not-exist'`.
2. Run again:

```bash
agentmesh workflows run demo.search --yes
```

The executor tries the cascade **selector → role+name → visible text →
name → semantics → bare role**, resolves even though the selector fails, and
leaves a **repair proposal** (`repair.proposed`) under `repairs/` without
rewriting your workflow. Repairs never apply themselves: you decide. Steps
with only positional selectors are flagged `[!]` in review with a note.
Without `aria-label`, the recorder uses whatever exists: visible text on
buttons/links and `placeholder` on inputs (e.g. `placeholder="Phone"`
resolves the field even if the DOM moves).

## 7. Second demonstration: turn differences into parameters

Record the same task with another value and learn it into the existing
workflow:

```bash
agentmesh teach --target browser \
  --name demo.search --headed \
  --start-url http://127.0.0.1:3221/page.html \
  --scope http://127.0.0.1:3221 \
  --continue demo.search
```

This time type `gamma`. When comparing, Teach detects the divergence
(`beta` → `gamma`) and offers to parameterize it:

- `Parameter name (empty to keep the stored literal)` → type `search`.
- You will see `parameterized as search` and then `Updated demo.search (0 new steps)`.

Inspect the YAML: the `ui.fill` step now says
`value: '{{ inputs.search }}'`. Run it with a third value to close the
loop:

```bash
agentmesh workflows run demo.search --input search=delta --yes
```

This is how a workflow grows with several demos, no LLM involved: only
deterministic comparison. (If the value was already a parameter, Teach answers
`No new parameter candidates; the demonstration matches ...`; genuinely new
steps — another op or another element — get appended to the workflow.)

## 8. Persistent sessions (log in once)

For apps with login (WhatsApp, GitHub, ...):

```bash
agentmesh sessions login whatsapp --url https://web.whatsapp.com
```

The browser opens (it uses `--headed` if you need it to scan the QR);
log in and finish with Enter. The profile stays stored. Check it and
use it to record or run without logging in again:

```bash
agentmesh sessions list
agentmesh teach --target browser --session whatsapp --name wa.read \
  --start-url https://web.whatsapp.com --scope https://web.whatsapp.com
agentmesh workflows run wa.read --session whatsapp --yes
```

## 9. Secrets: never in the trace

If you fill a `<input type="password">` while recording, the trace does not
store the value: it stores `secret://<host>/<field>`. When reviewing the
draft, Teach asks for a name per secret:

- `Secret name for secret://www.linkedin.com/_R_abc123 [linkedin_password]`
  → Enter (or type your own, e.g. `my_key`). Ids like `_R_...`
  change on every load, so your own name is what keeps replays stable.
  Repeated fills of the same field collapse into a single step.
- `Save a value for SECRET_LINKEDIN_PASSWORD to .env now? [y/N]` → `y` and
  type the value: it lands in `$AGENTMESH_HOME/.env` (mode 0600), never in the
  YAML. Runs load it on their own, with no re-export needed.

```bash
agentmesh secret list          # which references your workflows use and whether they are set
SECRET_LINKEDIN_PASSWORD=hunter2 agentmesh workflows run my.login --yes
```

Rules: secrets live in the local `.env` (gitignored) or environment
variables, never in traces, YAMLs, or audits. If the secret is missing, the
run fails closed.

> Enter-to-login: if you submit the form by pressing **Enter** inside the
> field (typical in logins), the recording captures it and replay presses the
> key (`ui.press` with `Enter`) instead of clicking the field — the click
> would submit nothing.

## 10. Compile to MCP and use it as a tool

Export the workflow as an MCP server (stdio):

```bash
agentmesh workflows export demo.search --target mcp --out /tmp/demo-mcp
cd /tmp/demo-mcp && npm install
AGENTMESH_MCP_ALLOW_WRITES=1 node server.mjs
```

The scaffold includes `server.mjs`, `package.json`, `memory.json`,
`workflows/demo.search.yaml`, and `schemas/demo.search.inputs.json`. It exposes one tool (e.g. `demo_search`) that
any MCP client can call; the scaffold's `README.md` explains how to
plug it in.

To give the model every capability you have taught, export them
as a single MCP library:

```bash
agentmesh workflows export --all --target mcp --out /tmp/my-capabilities-mcp
```

To plug the server into a client without hand-writing the file,
generate its ready-to-use configuration (`--profile` accepts the session
name or the profile directory; without it, tools run logged out):

```bash
agentmesh workflows client-config --client claude-code --server /tmp/my-capabilities-mcp/server.mjs --profile whatsapp --out /tmp/claude-mcp.json
agentmesh workflows client-config --client claude-desktop --server /tmp/my-capabilities-mcp/server.mjs --profile whatsapp --out /tmp/desktop.json
agentmesh workflows client-config --client generic --server /tmp/my-capabilities-mcp/server.mjs --out /tmp/mcp.json
```

For small libraries, each workflow appears as its own tool.
Above 40 capabilities the server enters compact mode and the model uses
`teach_search_capabilities`, `teach_describe_capability`, and
`teach_execute_capability`; that keeps hundreds of schemas out of context.
`teach_search_routines` retrieves repeated semantic sequences, with no values
or secrets, to help compose plans. Long jobs use
`teach_start_capability`, `teach_get_run`, and `teach_cancel_run`.

For safety, a write operation requests contextual MCP approval.
The owner can preauthorize it with `AGENTMESH_MCP_ALLOW_WRITES=1` only in a
controlled environment. `allowed_origins`, `allowed_operations`,
`denied_operations`, and `max_runs_per_hour` always apply; every call leaves
an audit trail, ARIA observations, experience, and repair proposals. Extracted
content is marked `untrusted_external`: it is data, never an instruction.

## 11. Memory, evaluation, and external training

Every run stores a redacted experience. Summarize real reliability:

```bash
agentmesh workflows report demo.search
agentmesh workflows report                 # every capability
agentmesh workflows report --since-days 7 --status failed
agentmesh workflows flaky                  # per-step reliability: retries, self-heals, repairs
agentmesh workflows list --lenient         # one corrupt YAML no longer hides the library
```

Evaluate a workflow against expected cases (useful in CI):

```bash
# cases.json: [{"inputs": {"search": "delta"}, "expect": {"results": ["found:delta"]}}]
agentmesh workflows eval demo.search --cases cases.json --yes
```

Export trajectories with workflow, result, and ARIA snapshots for evaluation,
RAG, or fine-tuning on an external system:

```bash
agentmesh workflows dataset --out /tmp/teach-trajectories.jsonl
agentmesh workflows dataset --workflow demo.search --status succeeded --limit 100 --out /tmp/subset.jsonl
```

Teach delivers operational intelligence immediately over MCP; this dataset
is the bridge for training model weights, which separately requires a
base model, a training provider, and evaluations. Input values
and secrets are not included.

## 12. Advanced recovery and other platforms

The IR supports `preconditions`, `success` verifiers, `failure` detectors,
bounded retries, checkpoints, ARIA snapshots, and screenshots. A
`recovery.vision_adapter` raises a visual request and, if you configure
`AGENTMESH_VISION_ADAPTER_<NAME>`, invokes the adapter; its proposal never
self-applies. The `desktop`, `mobile`, and `api` runtimes connect through
`AGENTMESH_RUNTIME_DESKTOP`, `AGENTMESH_RUNTIME_MOBILE`, and
`AGENTMESH_RUNTIME_API`. They receive `--ir` and `--inputs`, so the core does
not grant implicit universal access: each platform keeps its sandbox,
credentials, and policy.

The demonstration also stores sanitized shapes of observed APIs
(method, host, path, and query names). No bodies,
headers, cookies, tokens, or values are stored, and nothing runs as an API
automatically. Generate a starter adapter for human review and verify it meets
the contract (`--ir FILE --inputs JSON`):

```bash
agentmesh workflows gen-api-adapter demo.search --out /tmp/demo-api-adapter.sh
agentmesh workflows check-adapter --runtime api --adapter /tmp/demo-api-adapter.sh
```

## 12b. Lifecycle: diff, repairs, composition, and secrets

```bash
agentmesh workflows diff demo.search demo.search2   # compare steps/description/runtime
agentmesh workflows apply-repair demo.search --yes  # apply proposed healings (drop dead selectors)
agentmesh workflows relax demo.search              # generalize brittle asserts (volatile URLs, duplicates)
agentmesh workflows rename demo.search demo.find
agentmesh workflows import ./backup.yaml [--overwrite]
agentmesh workflows delete demo.find --yes          # keeps a revision in revisions/
agentmesh workflows compose --namespace common --name search   # promote the most repeated routine to a sub-workflow
agentmesh workflows prune --older-than-days 30 --keep-last 10 [--dry-run]
agentmesh replay <RUN_ID> --input search=other       # re-run with different inputs
agentmesh secret set SECRET_DEMO_KEY                 # stores in .env (0600); rotate does the same
```

Every save overwrites with a copy in `workflows/revisions/`, so teaching
over never loses the last valid document.

## 12c. MCP without restarts

Re-exporting (`workflows export --all`) rewrites `catalog.json` and
`routines.json`, which the running server reloads on its own (it watches the
files). You can also force it with the
`teach_reload_capabilities` tool. Only newly added per-tool registrations
in non-compact libraries require a restart.

## 13. Cleanup

```bash
# Delete the whole lab (workflows, runs, profiles, repairs)
rm -rf /tmp/teach-lab /tmp/teach-e2e
# Stop the demo server (Ctrl-C in its terminal)
```

Your real `~/.agentmesh` stays intact because everything ran under `AGENTMESH_HOME`.

## Troubleshooting

| Symptom | Likely cause and fix |
|---|---|
| `Teach runtime not found` | The TS package is not built: `cd teach && npm install && npm run build`, or export `AGENTMESH_TEACH_DIST=/path/to/teach/dist` |
| `refusing to record: trace ... is owned by live recorder PID` | An orphaned recorder is left (terminal force-closed). Kill it (`Ctrl-C` in its terminal or `kill <PID>`) and retry |
| `CDP port ... already answers` | An orphaned Chrome holds the port. Kill it or use another `--cdp-port` |
| `teach is interactive; run it in a terminal` | `teach` needs a TTY: run it in a terminal, not in a pipe |
| `no interactions recorded` | You stopped recording without acting on the page: interact first, Enter after |
| Browser does not open / Chrome error | Install stable Google Chrome (the recorder uses the system Chrome, it downloads none) |
