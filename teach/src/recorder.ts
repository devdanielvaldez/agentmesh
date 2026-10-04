/**
 * AgentMesh Teach browser recorder.
 *
 * Drives installed Google Chrome through Playwright (CDP under the hood),
 * captures semantic interaction events — never coordinates, never secrets —
 * and appends them as JSONL semantic-trace events.
 *
 * Usage:
 *   node dist/recorder.js --profile DIR --trace FILE [--scope ORIGIN]
 *     [--start-url URL] [--headed] [--smoke]
 *
 * Stop with SIGINT/SIGTERM/SIGHUP or by writing to --stop-file;
 * a `session.stop` event is always written (SIGKILL cannot).
 * Startup is exclusive per trace file: a live lockfile or an answering
 * --cdp-port refuses to start so two recorders can never interleave.
 */

import { appendFileSync, mkdirSync, readFileSync, unlinkSync, writeFileSync } from "node:fs";
import { connect } from "node:net";
import { dirname } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium } from "playwright-core";
import type { Args, InPageTarget } from "./recorder.types.js";

function arg(name: string): string | undefined {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
}

function parseArgs(): Args {
  const profile = arg("--profile");
  const trace = arg("--trace");
  if (!profile || !trace) {
    console.error("usage: recorder.js --profile DIR --trace FILE [--scope ORIGIN] [--start-url URL] [--headed] [--smoke]");
    process.exit(2);
  }
  const cdpPort = arg("--cdp-port");
  return {
    profile,
    trace,
    scope: arg("--scope") ?? "",
    startUrl: arg("--start-url") ?? "",
    headed: process.argv.includes("--headed"),
    smoke: process.argv.includes("--smoke"),
    verbose: process.argv.includes("--verbose"),
    cdpPort: cdpPort ? Number(cdpPort) : 0,
  };
}

export const RECORDER_INIT_SCRIPT = `
(() => {
  if (window.__amInstalled) return;
  window.__amInstalled = true;

  function cssPath(el) {
    const parts = [];
    let node = el;
    while (node && node.nodeType === 1 && parts.length < 6) {
      let part = node.tagName.toLowerCase();
      const testId = node.getAttribute && node.getAttribute("data-testid");
      const id = node.id;
      const name = node.getAttribute && node.getAttribute("name");
      if (testId) { parts.unshift('[data-testid="' + testId + '"]'); break; }
      if (id && !/^[0-9]/.test(id)) { parts.unshift("#" + id); break; }
      if (name && /^(input|select|textarea|button)$/.test(part)) {
        parts.unshift(part + '[name="' + name + '"]'); break;
      }
      let sib = node.previousElementSibling, nth = 1;
      while (sib) { if (sib.tagName === node.tagName) nth++; sib = sib.previousElementSibling; }
      parts.unshift(part + ":nth-of-type(" + nth + ")");
      node = node.parentElement;
    }
    return parts.join(" > ");
  }

  function implicitRole(el, tag, type) {
    if (tag === "input") {
      if (type === "checkbox" || type === "radio") return type;
      if (type === "submit" || type === "button" || type === "image") return "button";
      if (type === "hidden") return "";
      return "textbox";
    }
    if (tag === "textarea") return "textbox";
    if (tag === "button") return "button";
    if (tag === "select") return "combobox";
    if (tag === "a" && el.hasAttribute("href")) return "link";
    return "";
  }

  function describe(el) {
    const tag = el.tagName.toLowerCase();
    const type = (el.getAttribute("type") || "").toLowerCase();
    const secret =
      type === "password" ||
      (el.getAttribute("autocomplete") || "").toLowerCase().includes("current-password");
    const text = (el.innerText || "").trim().slice(0, 200);
    const value = secret ? "" : String(el.value ?? "").slice(0, 500);
    return {
      tag,
      role: el.getAttribute("role") || implicitRole(el, tag, type),
      name: el.getAttribute("aria-label") || el.getAttribute("name") || "",
      placeholder: el.getAttribute("placeholder") || "",
      testId: el.getAttribute("data-testid") || "",
      id: el.id || "",
      text,
      inputType: (el.getAttribute("type") || "").toLowerCase(),
      autocomplete: (el.getAttribute("autocomplete") || "").toLowerCase(),
      value,
      secret,
      selectors: [cssPath(el)],
    };
  }

  async function emit(kind, el, extra) {
    try {
      await window.__amEvent(kind, { target: describe(el), ...extra });
    } catch { /* recorder detached */ }
  }

  // Alt+click marks "capture text here": it records an extraction probe
  // instead of a click, with a preview of the readable content.
  function extractPreview(el) {
    const items = [];
    if (/^(ul|ol|table|dl)$/.test(el.tagName.toLowerCase())) {
      const cells = el.querySelectorAll("li, td, th, dt, dd");
      for (let i = 0; i < cells.length && items.length < 20; i++) {
        const text = (cells[i].innerText || "").trim();
        if (text) items.push(text.slice(0, 200));
      }
    }
    if (items.length === 0) {
      const text = (el.innerText || "").trim().slice(0, 500);
      if (text) items.push(text);
    }
    return items;
  }

  document.addEventListener("click", (e) => {
    const el = e.target instanceof Element ? e.target : null;
    if (!el) return;
    if (e.altKey) {
      e.preventDefault();
      e.stopPropagation();
      void emit("ui.extract", el, { extracted: extractPreview(el) });
      return;
    }
    void emit("ui.click", el, {});
  }, true);

  document.addEventListener("input", (e) => {
    const el = e.target instanceof Element ? e.target : null;
    if (el && /^(input|textarea|select)$/.test(el.tagName.toLowerCase())) {
      void emit("ui.fill", el, {});
    }
  }, true);

  document.addEventListener("change", (e) => {
    const el = e.target instanceof Element ? e.target : null;
    if (el && el.tagName.toLowerCase() === "select") {
      const opt = el.selectedOptions && el.selectedOptions[0];
      void emit("ui.select", el, { selected: opt ? opt.text.trim().slice(0, 200) : "" });
    }
  }, true);

  document.addEventListener("submit", (e) => {
    const el = e.target instanceof Element ? e.target : null;
    if (el) void emit("ui.submit", el, {});
  }, true);

  // Enter inside a field usually submits the form (login especially), often
  // without any click: capture it so replay presses the key instead of
  // clicking the field (which would do nothing).
  document.addEventListener("keydown", (e) => {
    const el = e.target instanceof Element ? e.target : null;
    if (!el) return;
    if (e.key === "Enter" && /^(input|textarea|select)$/.test(el.tagName.toLowerCase())) {
      void emit("ui.submit", el, {});
    }
  }, true);

  // Wheel events point at a child inside the scrolling region. Walk toward
  // the document and keep the nearest ancestor that can actually scroll in
  // the gesture's dominant axis. A short debounce turns the many events from
  // a wheel/trackpad gesture into one semantic ui.scroll event.
  function scrollRegion(start, deltaX, deltaY) {
    const vertical = Math.abs(deltaY) >= Math.abs(deltaX);
    let el = start;
    while (el) {
      const style = getComputedStyle(el);
      const overflow = vertical ? style.overflowY : style.overflowX;
      const hasRange = vertical
        ? el.scrollHeight > el.clientHeight + 1
        : el.scrollWidth > el.clientWidth + 1;
      if (hasRange && /^(auto|scroll|overlay)$/.test(overflow)) return el;
      el = el.parentElement;
    }
    return document.scrollingElement || document.documentElement;
  }

  let pendingScroll = null;
  function flushScroll() {
    const pending = pendingScroll;
    pendingScroll = null;
    if (!pending) return;
    clearTimeout(pending.timer);
    if (Math.abs(pending.deltaX) + Math.abs(pending.deltaY) < 1) return;
    void emit("ui.scroll", pending.el, {
      deltaX: pending.deltaX,
      deltaY: pending.deltaY,
      untilStable: pending.untilStable,
    });
  }

  document.addEventListener("wheel", (e) => {
    const start = e.target instanceof Element ? e.target : document.documentElement;
    const scale = e.deltaMode === 1
      ? 16
      : e.deltaMode === 2
        ? Math.max(window.innerHeight, window.innerWidth)
        : 1;
    const deltaX = e.deltaX * scale;
    const deltaY = e.deltaY * scale;
    const el = scrollRegion(start, deltaX, deltaY);
    if (pendingScroll && pendingScroll.el !== el) flushScroll();
    if (!pendingScroll) {
      pendingScroll = { el, deltaX: 0, deltaY: 0, untilStable: false, timer: 0 };
    }
    pendingScroll.deltaX += deltaX;
    pendingScroll.deltaY += deltaY;
    pendingScroll.untilStable ||= e.altKey;
    clearTimeout(pendingScroll.timer);
    pendingScroll.timer = setTimeout(flushScroll, 180);
  }, { capture: true, passive: true });
})();
`;

function secretRef(host: string, target: InPageTarget): string {
  const field = target.name || target.id || target.tag;
  return `secret://${host}/${field}`;
}

/** A previous recorder owns this trace when its lockfile names a live PID. */
function traceOwner(trace: string): number | undefined {
  let raw: string;
  try {
    raw = readFileSync(trace + ".lock", "utf8");
  } catch {
    return undefined;
  }
  const pid = Number(JSON.parse(raw).pid);
  if (!Number.isInteger(pid) || pid <= 0) return undefined;
  try {
    process.kill(pid, 0);
    return pid;
  } catch {
    return undefined;
  }
}

/**
 * Claims the trace for this session: refuses when another live recorder
 * owns it (a second writer would interleave sequence numbers), otherwise
 * starts a fresh file so stale events can never leak into the new trace.
 */
let claimedTrace = "";
process.on("exit", () => {
  if (claimedTrace) {
    try {
      unlinkSync(claimedTrace + ".lock");
    } catch {
      // Already released by stop(); a stale lock is reaped anyway.
    }
  }
});

function claimTrace(trace: string): void {
  const owner = traceOwner(trace);
  if (owner !== undefined) {
    console.error(
      `refusing to record: trace ${trace} is owned by live recorder PID ${owner}; ` +
        `stop that session first (Ctrl-C) or delete ${trace}.lock if it is stale`,
    );
    process.exit(2);
  }
  try {
    unlinkSync(trace + ".lock");
  } catch {
    // No stale lock; nothing to clear.
  }
  writeFileSync(trace + ".lock", JSON.stringify({ pid: process.pid }) + "\n");
  claimedTrace = trace;
  writeFileSync(trace, "");
}

/** Fails fast when the requested CDP port already answers (likely an orphaned browser). */
async function checkCdpPort(port: number): Promise<void> {
  if (!port) return;
  const inUse = await new Promise<boolean>((resolve) => {
    const socket = connect(port, "127.0.0.1");
    socket.once("connect", () => {
      socket.destroy();
      resolve(true);
    });
    socket.once("error", () => resolve(false));
  });
  if (inUse) {
    console.error(
      `refusing to record: CDP port ${port} already answers; another (possibly orphaned) ` +
        `browser is using it. Kill it or pass a different --cdp-port.`,
    );
    process.exit(2);
  }
}

/**
 * One live log line per captured event for the recording terminal.
 * Reads only the post-redaction event: secret fills carry a `secret_ref`,
 * never a value, so values printed here are safe by construction.
 */
export function describeEvent(event: Record<string, unknown>): string {
  const kind = String(event.kind ?? "unknown");
  const str = (value: unknown): string => (typeof value === "string" ? value : "");
  const clip = (text: string, max = 80): string =>
    text.length > max ? text.slice(0, max - 1) + "…" : text;
  if (kind === "browser.navigate") return `${kind} ${clip(str(event.url), 120)}`;
  if (kind === "network.call") {
    return `${kind} ${str(event.method)} ${str(event.host)}${str(event.path)}`;
  }
  if (kind === "network.response") {
    return `${kind} ${String(event.status ?? "?")} ${str(event.host)}${str(event.path)}`;
  }
  if (kind === "session.start") return `${kind} scope=${str(event.scope)}`;
  if (kind === "session.stop") return kind;
  const target = (event.target ?? {}) as Record<string, unknown>;
  const tag = str(target.tag);
  const role = str(target.role);
  const who = [
    tag,
    role && role !== tag ? role : "",
    str(target.name) && `"${str(target.name)}"`,
    str(target.text) && `"${clip(str(target.text), 40)}"`,
    str(target.placeholder) && `placeholder=${JSON.stringify(str(target.placeholder))}`,
    str(target.autocomplete) && `autocomplete=${JSON.stringify(str(target.autocomplete))}`,
    (target.selectors as string[] | undefined)?.[0],
  ]
    .filter(Boolean)
    .join(" ");
  let what = "";
  const value = (event.value ?? {}) as Record<string, unknown>;
  if (typeof value.literal === "string") what = ` = ${clip(JSON.stringify(value.literal), 60)}`;
  else if (typeof value.secret_ref === "string") what = ` = ${value.secret_ref}`;
  else if (typeof event.selected === "string") what = ` = ${clip(JSON.stringify(event.selected), 60)}`;
  else if (typeof event.extracted === "object") {
    what = ` (${(event.extracted as unknown[]).length} items)`;
  } else if (kind === "ui.scroll") {
    const deltaX = Number(event.deltaX ?? 0);
    const deltaY = Number(event.deltaY ?? 0);
    const direction = Math.abs(deltaY) >= Math.abs(deltaX)
      ? (deltaY < 0 ? "up" : "down")
      : (deltaX < 0 ? "left" : "right");
    what = ` ${direction}${event.untilStable === true ? " until_stable" : ""}`;
  }
  return `${kind}${who ? " " + who : ""}${what}`;
}

async function main(): Promise<void> {
  const args = parseArgs();
  mkdirSync(dirname(args.trace), { recursive: true });
  claimTrace(args.trace);
  const stopFile = arg("--stop-file");
  if (stopFile) {
    try {
      unlinkSync(stopFile);
    } catch {
      // A leftover stop signal must never end the new session instantly.
    }
  }
  await checkCdpPort(args.cdpPort);

  let seq = 0;
  const emit = (event: Record<string, unknown>): void => {
    seq += 1;
    appendFileSync(
      args.trace,
      JSON.stringify({ seq, ts_ms: Date.now(), ...event }) + "\n",
    );
    if (args.verbose) {
      console.error(`recorded #${seq} ${describeEvent(event)}`);
    }
  };

  const scopeHost = args.scope ? new URL(args.scope).host : "";
  const launchArgs = ["--no-first-run", "--no-default-browser-check"];
  if (args.cdpPort) {
    launchArgs.push(`--remote-debugging-port=${args.cdpPort}`);
  }
  const context = await chromium.launchPersistentContext(args.profile, {
    channel: "chrome",
    headless: !args.headed,
    args: launchArgs,
  });
  const emitAction = async (kind: string, payload: Record<string, unknown>): Promise<void> => {
    const page = payload.page as { url(): string } | undefined;
    const url = typeof page?.url === "function" ? page.url() : "";
    const host = url.startsWith("http") ? new URL(url).host : "";
    const target = payload.target as InPageTarget;
    if (kind === "ui.fill" && target.secret) {
      emit({
        kind,
        url,
        out_of_scope: scopeHost !== "" && host !== scopeHost,
        target: { ...target, value: "" },
        value: { secret_ref: secretRef(host || "unknown", target) },
      });
      return;
    }
    emit({
      kind,
      url,
      out_of_scope: scopeHost !== "" && host !== scopeHost,
      target,
      ...(payload.selected !== undefined ? { selected: payload.selected } : {}),
      ...(kind === "ui.fill" ? { value: { literal: target.value } } : {}),
      ...(kind === "ui.extract" && Array.isArray(payload.extracted)
        ? { extracted: (payload.extracted as string[]).slice(0, 20) }
        : {}),
      ...(kind === "ui.scroll"
        ? {
            deltaX: Number.isFinite(Number(payload.deltaX)) ? Number(payload.deltaX) : 0,
            deltaY: Number.isFinite(Number(payload.deltaY)) ? Number(payload.deltaY) : 0,
            untilStable: payload.untilStable === true,
          }
        : {}),
    });
  };

  await context.exposeBinding("__amEvent", async (_source, kind: string, payload: Record<string, unknown>) => {
    const page = _source.page as unknown as { url(): string };
    await emitAction(kind, { ...payload, page });
  });
  await context.addInitScript(RECORDER_INIT_SCRIPT);
  // Pages open before the capture script installed never received it;
  // reloading gives them a fresh document with capture active.
  for (const page of context.pages()) {
    await page.reload({ waitUntil: "domcontentloaded" }).catch(() => undefined);
  }

  for (const page of context.pages()) {
    page.on("framenavigated", (frame) => {
      if (frame === page.mainFrame()) {
        emit({ kind: "browser.navigate", url: frame.url() });
      }
    });
  }
  context.on("page", (page) => {
    page.on("framenavigated", (frame) => {
      if (frame === page.mainFrame()) {
        emit({ kind: "browser.navigate", url: frame.url() });
      }
    });
    page.on("request", (request) => {
      const url = new URL(request.url());
      if (url.protocol !== "http:" && url.protocol !== "https:") return;
      const keys = [...url.searchParams.keys()];
      emit({
        kind: "network.call",
        method: request.method(),
        host: url.host,
        path: url.pathname,
        query_keys: keys,
      });
    });
    page.on("response", (response) => {
      const url = new URL(response.url());
      if (url.protocol !== "http:" && url.protocol !== "https:") return;
      emit({
        kind: "network.response",
        host: url.host,
        path: url.pathname,
        status: response.status(),
      });
    });
  });

  emit({
    kind: "session.start",
    scope: args.scope || null,
    headed: args.headed,
  });

  let stopped = false;
  const releaseLock = (): void => {
    try {
      unlinkSync(args.trace + ".lock");
    } catch {
      // Best effort: a stale lock is reaped by the next session.
    }
  };
  const stop = async (): Promise<void> => {
    if (stopped) return;
    stopped = true;
    emit({ kind: "session.stop" });
    await context.close();
    releaseLock();
  };
  const shutdown = (code: number): void => {
    void stop()
      .catch(() => undefined)
      .then(() => process.exit(code));
  };
  process.on("SIGINT", () => shutdown(0));
  process.on("SIGTERM", () => shutdown(0));
  process.on("SIGHUP", () => shutdown(0));

  // Optional cooperative stop: the CLI writes any content here when the
  // user finishes, so shutdown always writes session.stop (SIGKILL cannot).
  if (stopFile) {
    const timer = setInterval(() => {
      try {
        if (readFileSync(stopFile, "utf8").trim() !== "") {
          clearInterval(timer);
          void stop().then(() => process.exit(0));
        }
      } catch {
        // Missing file means keep recording.
      }
    }, 500);
    timer.unref?.();
  }

  if (args.smoke) {
    await runSmoke(context, emit);
    await stop();
    return;
  }

  const page = context.pages()[0] ?? (await context.newPage());
  if (args.startUrl) {
    await page.goto(args.startUrl, { waitUntil: "domcontentloaded" });
  }
  console.error(`Recording to ${args.trace}. Interact in the browser; stop with Ctrl-C.`);
  await new Promise(() => undefined);
}

/** Self-contained capture check: drives a data-URL form through the real pipeline. */
async function runSmoke(
  context: Awaited<ReturnType<typeof chromium.launchPersistentContext>>,
  emit: (event: Record<string, unknown>) => void,
): Promise<void> {
  const page = context.pages()[0] ?? (await context.newPage());
  // Navigate first so the capture script installs, then inject content with
  // innerHTML: unlike setContent it keeps the same document, so the capture
  // listeners survive. The form never submits anywhere.
  await page.goto("about:blank");
  await page.evaluate(() => {
    document.body.innerHTML = `
    <form id="login" onsubmit="return false">
      <input name="contact" aria-label="Contact" />
      <input name="pw" type="password" />
      <button type="submit">Send</button>
    </form>
    <ul id="messages"><li>hello</li><li>world</li></ul>
    <div id="feed" role="feed" aria-label="Messages"
         style="height:80px; overflow-y:auto">
      <div style="height:500px">scroll content</div>
    </div>`;
  });
  await page.getByLabel("Contact").fill("Daniel");
  await page.locator('input[name="pw"]').fill("s3cret");
  await page.getByRole("button", { name: "Send" }).click();
  // Alt+click is the "capture text here" gesture: it must record an
  // extraction probe through the same in-page pipeline as real users.
  await page.locator("#messages").click({ modifiers: ["Alt"] });
  const feed = page.locator("#feed");
  await feed.hover();
  await page.mouse.wheel(0, 240);
  await page.waitForTimeout(300);
  const items = await page.locator("#messages li").allInnerTexts();
  emit({ kind: "ui.extract", url: page.url(), target: { semantic: "messages" }, extracted: items });
  console.error(`SMOKE_OK extracted=${items.length}`);
}

// Importable for tests (`node --test tests/`); the CLI runs only on direct invocation.
const invokedAsCli = (() => {
  try {
    return (
      process.argv[1] !== undefined &&
      import.meta.url === pathToFileURL(process.argv[1]).href
    );
  } catch {
    return false;
  }
})();

if (invokedAsCli) {
  void main().catch((error) => {
    console.error(`recorder failed: ${String(error)}`);
    process.exit(1);
  });
}
