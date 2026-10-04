/**
 * AgentMesh Teach workflow executor.
 *
 * Runs a Workflow IR document with Playwright: substitutes inputs, resolves
 * targets through a healing cascade (selector → role/name → text), enforces
 * side-effect confirmation, redacts secrets, and writes an audit trail plus
 * repair candidates.
 *
 * Usage:
 *   node dist/executor.js --ir FILE --inputs JSON [--profile DIR]
 *     [--headed] [--dry-run] [--yes] [--audit FILE] [--repair-dir DIR]
 *
 * Exit codes: 0 ok · 2 confirmation/policy denial · 3 target or runtime
 * failure. Result JSON goes to stdout; everything else to stderr.
 */

import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, type Browser, type BrowserContext, type Locator, type Page } from "playwright-core";
import yaml from "js-yaml";
import type {
  IrAssertion,
  IrStep,
  IrTarget,
  IrWorkflow,
  Resolution,
  StepOutcome,
} from "./executor.types.js";

export type { StepOutcome } from "./executor.types.js";

/** Parses IR as JSON, falling back to YAML (the stored format). */
function parseIr(raw: string): IrWorkflow {
  try {
    return JSON.parse(raw) as IrWorkflow;
  } catch {
    return yaml.load(raw) as IrWorkflow;
  }
}

/** First open page, or a fresh one, in a browser context. */
async function firstPage(context: BrowserContext): Promise<Page> {
  return context.pages()[0] ?? (await context.newPage());
}

const WRITE_OPS = new Set([
  "ui.fill",
  "ui.click",
  "ui.activate",
  "ui.press",
  "ui.select",
  "ui.drag",
  "ui.drop",
  "ui.focus",
  "ui.scroll",
  "file.choose",
  "file.upload",
  "file.download",
  "file.save",
  "app.open",
  "app.close",
  "app.focus",
  "auth.require_user",
  "human.confirm",
  "human.authenticate",
  "human.resolve",
]);

const READ_OPS = new Set([
  "browser.navigate",
  "browser.back",
  "browser.forward",
  "browser.new_tab",
  "browser.close_tab",
  "browser.wait_navigation",
  "ui.find",
  "ui.extract",
  "ui.wait",
  "ui.focus",
  "ui.scroll",
  "assert.exists",
  "assert.not_exists",
  "assert.text",
  "assert.url",
  "assert.schema",
  "assert.state",
  "auth.ensure_session",
  "auth.request_secret",
  "control.if",
  "control.switch",
  "control.loop",
  "control.retry",
  "control.timeout",
]);

/** Matches an exact operation or a family wildcard such as `ui.*`. */
export function operationMatches(rule: string, operation: string): boolean {
  return rule === operation || (rule.endsWith(".*") && operation.startsWith(rule.slice(0, -1)));
}

/** Enforces the operation allow/deny lists before the browser performs a step. */
export function assertOperationAllowed(workflow: IrWorkflow, step: IrStep): void {
  const policy = workflow.policy;
  if (!policy) return;
  if ((policy.denied_operations ?? []).some((rule) => operationMatches(rule, step.op))) {
    throw new Error(`POLICY_DENIED: operation "${step.op}" is denied`);
  }
  const allowed = policy.allowed_operations ?? [];
  if (allowed.length > 0 && !allowed.some((rule) => operationMatches(rule, step.op))) {
    throw new Error(`POLICY_DENIED: operation "${step.op}" is not allowed`);
  }
}

/** Exact URL-origin check (scheme + host + port), used before and after actions. */
export function originAllowed(url: string, allowedOrigins: string[]): boolean {
  try {
    const actual = new URL(url);
    if (actual.protocol !== "http:" && actual.protocol !== "https:") return true;
    return allowedOrigins.some((allowed) => new URL(allowed).origin === actual.origin);
  } catch {
    return false;
  }
}

function arg(name: string): string | undefined {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
}

function flag(name: string): boolean {
  return process.argv.includes(name);
}

/** Keeps only starts in the rolling hour and reports whether another run fits. */
export function rateWindow(
  timestamps: number[],
  limit: number,
  now = Date.now(),
): { allowed: boolean; active: number[] } {
  const active = timestamps.filter((timestamp) => timestamp > now - 60 * 60 * 1000 && timestamp <= now);
  return { allowed: active.length < limit, active };
}

function renderValue(value: unknown): string {
  return typeof value === "string" ? value : JSON.stringify(value ?? "");
}

export function substitute(
  template: string,
  inputs: Record<string, unknown>,
  results: Record<string, unknown> = {},
): string {
  const withInputs = template.replace(/\{\{\s*inputs\.([A-Za-z0-9_]+)\s*\}\}/g, (_m, name: string) => {
    if (!(name in inputs)) {
      throw new Error(`missing input "${name}"`);
    }
    return renderValue(inputs[name]);
  });
  return withInputs.replace(/\{\{\s*steps\.([A-Za-z0-9_]+)(?:\.result)?\s*\}\}/g, (_m, id: string) => {
    if (!(id in results)) {
      throw new Error(`missing step result "${id}"`);
    }
    return renderValue(results[id]);
  });
}

/** Falsy renders: empty, false, 0, no, off, null, undefined, none (case-insensitive). */
export function evalCondition(rendered: string): boolean {
  const normalized = rendered.trim().toLowerCase();
  return !["", "false", "0", "no", "off", "null", "undefined", "none"].includes(normalized);
}

/** Parses a rendered iteration count, bounded to 1–100. */
export function parseIterations(rendered: string, stepId: string): number {
  const count = Number(rendered.trim());
  if (!Number.isInteger(count) || count < 1 || count > 100) {
    throw new Error(`step "${stepId}" (control.loop) needs iterations 1-100, got ${JSON.stringify(rendered)}`);
  }
  return count;
}

export type ScrollDirection = "up" | "down" | "left" | "right";

/** Parses the ui.scroll iteration contract without allowing an unbounded run. */
export function parseScrollIterations(
  rendered: string,
  stepId: string,
): number | "until_stable" {
  if (rendered.trim() === "until_stable") return "until_stable";
  const count = Number(rendered.trim());
  if (!Number.isInteger(count) || count < 1 || count > 100) {
    throw new Error(
      `step "${stepId}" (ui.scroll) needs iterations 1-100 or until_stable, got ${JSON.stringify(rendered)}`,
    );
  }
  return count;
}

/** One gesture moves most of the visible region while retaining context. */
export function scrollDelta(
  direction: ScrollDirection,
  clientWidth: number,
  clientHeight: number,
): { left: number; top: number } {
  const horizontal = Math.max(100, Math.floor((clientWidth || 750) * 0.8));
  const vertical = Math.max(100, Math.floor((clientHeight || 750) * 0.8));
  switch (direction) {
    case "up": return { left: 0, top: -vertical };
    case "down": return { left: 0, top: vertical };
    case "left": return { left: -horizontal, top: 0 };
    case "right": return { left: horizontal, top: 0 };
  }
}

interface ScrollSnapshot {
  left: number;
  top: number;
  clientWidth: number;
  clientHeight: number;
  scrollWidth: number;
  scrollHeight: number;
}

async function scrollSnapshot(locator: Locator): Promise<ScrollSnapshot> {
  return locator.evaluate((node) => {
    const element = node as HTMLElement;
    const root = document.scrollingElement as HTMLElement | null;
    const target = element === document.body || element === document.documentElement
      ? (root ?? document.documentElement)
      : element;
    return {
      left: target.scrollLeft,
      top: target.scrollTop,
      clientWidth: target.clientWidth,
      clientHeight: target.clientHeight,
      scrollWidth: target.scrollWidth,
      scrollHeight: target.scrollHeight,
    };
  });
}

function scrollChanged(before: ScrollSnapshot, after: ScrollSnapshot): boolean {
  return before.left !== after.left ||
    before.top !== after.top ||
    before.scrollWidth !== after.scrollWidth ||
    before.scrollHeight !== after.scrollHeight;
}

/** Replays scroll against the recorded region, including bounded infinite pagination. */
export async function performScroll(
  locator: Locator,
  page: Page,
  direction: ScrollDirection,
  iterations: number | "until_stable",
  timeout: number,
  audit: (event: Record<string, unknown>) => void = () => undefined,
): Promise<number> {
  await locator.scrollIntoViewIfNeeded({ timeout });
  const maximum = iterations === "until_stable" ? 100 : iterations;
  let performed = 0;
  for (let round = 1; round <= maximum; round += 1) {
    const before = await scrollSnapshot(locator);
    const delta = scrollDelta(direction, before.clientWidth, before.clientHeight);
    await locator.evaluate((node, movement) => {
      const element = node as HTMLElement;
      const root = document.scrollingElement as HTMLElement | null;
      if (element === document.body || element === document.documentElement || element === root) {
        window.scrollBy({ left: movement.left, top: movement.top, behavior: "auto" });
      } else {
        element.scrollBy({ left: movement.left, top: movement.top, behavior: "auto" });
      }
    }, delta);
    performed = round;
    await page.waitForTimeout(200);
    let after = await scrollSnapshot(locator);
    let changed = scrollChanged(before, after);

    // Infinite feeds commonly append after the scroll position has already
    // stopped. Give one loading window before declaring the region stable.
    if (iterations === "until_stable" && !changed) {
      await page.waitForTimeout(600);
      const settled = await scrollSnapshot(locator);
      changed = scrollChanged(after, settled);
      after = settled;
    }
    audit({
      event: "step.scrolled",
      direction,
      round,
      changed,
      scroll_left: after.left,
      scroll_top: after.top,
    });
    if (iterations === "until_stable" && !changed) break;
  }
  return performed;
}

/** Checks a JSON value against a type descriptor (`array`, `object`, `string`, ...). */
export function assertSchemaMatches(expected: string, actual: unknown, stepId: string): void {
  const trimmed = expected.trim();
  let spec: { type?: string; minItems?: number } = { type: trimmed.toLowerCase() };
  if (trimmed.startsWith("{")) {
    try {
      spec = JSON.parse(trimmed) as { type?: string; minItems?: number };
    } catch {
      throw new Error(`step "${stepId}" (assert.schema) holds invalid JSON`);
    }
  }
  const type = (spec.type ?? trimmed).toLowerCase();
  const actualType = Array.isArray(actual) ? "array" : actual === null ? "null" : typeof actual;
  if (type && type !== "any" && actualType !== type) {
    throw new Error(`POSTCONDITION_FAILED: step "${stepId}" expected ${type}, got ${actualType}`);
  }
  if (spec.minItems !== undefined && (!Array.isArray(actual) || actual.length < spec.minItems)) {
    throw new Error(`POSTCONDITION_FAILED: step "${stepId}" expected at least ${spec.minItems} items`);
  }
}

function secretValue(ref: string): string {
  const name = "SECRET_" + ref.replace(/^secret:\/\//, "").replace(/[^A-Za-z0-9]+/g, "_").toUpperCase();
  const value = process.env[name];
  if (!value) {
    throw new Error(`secret unavailable: ${ref} (set ${name})`);
  }
  return value;
}

/** Waits until the page URL contains the expected fragment (redirects and
 * SPA route changes land asynchronously after the interaction that caused
 * them), then fails closed with the same shape as the old instant check. */
export async function assertUrl(
  page: Pick<Page, "url">,
  expected: string,
  timeoutMs: number,
): Promise<void> {
  const start = Date.now();
  for (;;) {
    if (page.url().includes(expected)) return;
    if (Date.now() - start > timeoutMs) {
      throw new Error(`POSTCONDITION_FAILED: url ${page.url()} lacks ${expected}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
}

export async function resolveTarget(
  page: Page,
  target: IrTarget,
  audit: (event: Record<string, unknown>) => void,
  waitMs = 5000,
): Promise<{ locator: Locator; resolution: Resolution }> {
  const candidates: Array<{ strategy: string; confidence: number; make: () => Locator; describe: string }> = [];
  for (const selector of target.selectors ?? []) {
    candidates.push({
      strategy: "selector",
      confidence: 0.97,
      make: () => page.locator(selector).first(),
      describe: selector,
    });
  }
  if (target.role && target.accessible_name) {
    const role = target.role as Parameters<Page["getByRole"]>[0];
    candidates.push({
      strategy: "role",
      confidence: 0.93,
      make: () => page.getByRole(role, { name: target.accessible_name }).first(),
      describe: `role=${target.role} name=${target.accessible_name}`,
    });
  }
  if (target.text) {
    const text = target.text;
    candidates.push({
      strategy: "text",
      confidence: 0.9,
      make: () => page.getByText(text, { exact: false }).first(),
      describe: `text~=${text.length > 60 ? text.slice(0, 59) + "…" : text}`,
    });
  }
  if (target.accessible_name && !target.role) {
    candidates.push({
      strategy: "name",
      confidence: 0.88,
      make: () => page.getByLabel(target.accessible_name as string).first(),
      describe: `label=${target.accessible_name}`,
    });
  }
  if (target.placeholder) {
    candidates.push({
      strategy: "placeholder",
      confidence: 0.87,
      make: () => page.getByPlaceholder(target.placeholder as string).first(),
      describe: `placeholder=${target.placeholder}`,
    });
  }
  if (target.autocomplete) {
    // Stable for login fields whose ids rotate on every page load
    // (React `useId` selectors like `#_r_3_` go stale immediately).
    const token = target.autocomplete as string;
    candidates.push({
      strategy: "autocomplete",
      confidence: 0.86,
      make: () => page.locator(`[autocomplete="${token}"]`).first(),
      describe: `autocomplete=${token}`,
    });
  }
  if (target.semantic) {
    candidates.push({
      strategy: "semantic",
      confidence: 0.82,
      make: () => page.getByText(target.semantic as string, { exact: false }).first(),
      describe: `text~=${target.semantic}`,
    });
  }
  if (target.role && !target.accessible_name) {
    // A bare role is a blind first-match: allow it only when the recorded
    // target carries no stronger signal. If text or a name was recorded but
    // matched nothing, the element genuinely differs, so clicking the first
    // button would diverge the flow — fail closed instead.
    const strongerSignalUnmatched = Boolean(target.text) || Boolean(target.accessible_name);
    if (!strongerSignalUnmatched) {
      const role = target.role as Parameters<Page["getByRole"]>[0];
      candidates.push({
        strategy: "role",
        confidence: 0.8,
        make: () => page.getByRole(role, {}).first(),
        describe: `role=${target.role} name=*`,
      });
    } else {
      audit({
        event: "target.blind_match_refused",
        target: `role=${target.role} (recorded text/name matched nothing)`,
      });
    }
  }
  let lastError = "";
  for (const candidate of candidates) {
    try {
      const locator = candidate.make();
      await locator.waitFor({ state: "visible", timeout: waitMs });
      audit({ event: "target.resolved", strategy: candidate.strategy, confidence: candidate.confidence, target: candidate.describe });
      return { locator, resolution: { strategy: candidate.strategy, confidence: candidate.confidence, describe: candidate.describe } };
    } catch (error) {
      lastError = String(error).split("\n")[0];
    }
  }
  throw new Error(`TARGET_NOT_FOUND (${lastError})`);
}

async function assertionMatches(
  page: Page,
  assertion: IrAssertion,
  inputs: Record<string, unknown>,
  results: Record<string, unknown>,
  audit: (event: Record<string, unknown>) => void,
  wait: boolean,
): Promise<boolean> {
  const value = assertion.value !== undefined ? substitute(assertion.value, inputs, results) : undefined;
  const expectedUrl = assertion.url !== undefined ? substitute(assertion.url, inputs, results) : value;
  const timeout = wait ? (assertion.timeout_ms ?? 5000) : Math.min(assertion.timeout_ms ?? 250, 250);
  switch (assertion.op) {
    case "assert.url": {
      if (!expectedUrl) return false;
      if (!wait) return page.url().includes(expectedUrl);
      try {
        await assertUrl(page, expectedUrl, timeout);
        return true;
      } catch {
        return false;
      }
    }
    case "assert.text": {
      const body = await page.locator("body").innerText({ timeout }).catch(() => "");
      return body.includes(value ?? "");
    }
    case "assert.exists": {
      if (!assertion.target) return false;
      return resolveTarget(page, assertion.target, audit, timeout).then(() => true).catch(() => false);
    }
    case "assert.not_exists": {
      if (!assertion.target) return false;
      return resolveTarget(page, assertion.target, audit, timeout).then(() => false).catch(() => true);
    }
  }
}

async function requireAssertions(
  label: string,
  assertions: IrAssertion[],
  page: Page,
  inputs: Record<string, unknown>,
  results: Record<string, unknown>,
  audit: (event: Record<string, unknown>) => void,
): Promise<void> {
  for (const assertion of assertions) {
    const matched = await assertionMatches(page, assertion, inputs, results, audit, true);
    audit({ event: "assertion.checked", phase: label, op: assertion.op, matched });
    if (!matched) throw new Error(`POSTCONDITION_FAILED: ${label} ${assertion.op}`);
  }
}

function safeArtifactName(value: string): string {
  return value.replace(/[^A-Za-z0-9_.-]+/g, "_").slice(0, 100);
}

/** Removes caller-provided values before an observation becomes durable memory. */
export function redactObservation(
  snapshot: string,
  inputs: Record<string, unknown>,
): string {
  const replacements = Object.entries(inputs)
    .map(([name, value]) => [name, typeof value === "string" ? value : JSON.stringify(value)] as const)
    .filter((entry): entry is readonly [string, string] => Boolean(entry[1]))
    .sort((left, right) => right[1].length - left[1].length);
  return replacements.reduce(
    (redacted, [name, value]) => redacted.split(value).join(`{{ input.${name} }}`),
    snapshot,
  );
}

async function captureAria(
  page: Page,
  artifactDir: string,
  label: string,
  inputs: Record<string, unknown>,
  audit: (event: Record<string, unknown>) => void,
): Promise<string | undefined> {
  if (!artifactDir) return undefined;
  try {
    mkdirSync(artifactDir, { recursive: true });
    const path = join(artifactDir, `${safeArtifactName(label)}.aria.yml`);
    const snapshot = await page.locator("body").ariaSnapshot({ timeout: 5000 });
    writeFileSync(path, redactObservation(snapshot, inputs));
    audit({ event: "observation.aria", label, path });
    return path;
  } catch (error) {
    audit({ event: "observation.failed", label, error: String(error).split("\n")[0] });
    return undefined;
  }
}

function retryable(error: unknown): boolean {
  const message = String(error instanceof Error ? error.message : error);
  return message.includes("TARGET_NOT_FOUND") || message.includes("Timeout") || message.includes("detached");
}

async function executeStep(
  page: Page,
  workflow: IrWorkflow,
  step: IrStep,
  inputs: Record<string, unknown>,
  results: Record<string, unknown>,
  yes: boolean,
  audit: (event: Record<string, unknown>) => void,
): Promise<StepOutcome> {
  const value = step.value !== undefined ? substitute(step.value, inputs, results) : undefined;
  const limit = step.limit !== undefined ? substitute(step.limit, inputs, results) : undefined;
  const url = step.url !== undefined ? substitute(step.url, inputs, results) : undefined;
  const filePath = step.path !== undefined ? substitute(step.path, inputs, results) : undefined;
  const timeout = step.timeout_ms ?? 15000;
  const primary = step.op.startsWith("ui.") && step.op !== "ui.wait" ? step.target : undefined;
  let resolution: Resolution | undefined;
  let activePage: Page | undefined;
  const current = (): Page => activePage ?? page;
  const locate = async (): Promise<Locator> => {
    if (!primary) throw new Error(`step "${step.id}" needs a target`);
    const resolved = await resolveTarget(current(), primary, audit);
    resolution = resolved.resolution;
    return resolved.locator;
  };

  switch (step.op) {
    case "browser.navigate": {
      if (!url) throw new Error(`step "${step.id}" needs a url`);
      if (workflow.policy?.allowed_origins?.length && !originAllowed(url, workflow.policy.allowed_origins)) {
        throw new Error(`ORIGIN_VIOLATION: ${new URL(url).origin}`);
      }
      await current().goto(url, { waitUntil: "domcontentloaded", timeout });
      break;
    }
    case "browser.back": await current().goBack({ timeout }); break;
    case "browser.forward": await current().goForward({ timeout }); break;
    case "browser.new_tab": {
      const tab = await page.context().newPage();
      if (url) {
        if (workflow.policy?.allowed_origins?.length && !originAllowed(url, workflow.policy.allowed_origins)) {
          throw new Error(`ORIGIN_VIOLATION: ${new URL(url).origin}`);
        }
        await tab.goto(url, { waitUntil: "domcontentloaded", timeout });
      }
      await tab.bringToFront();
      activePage = tab;
      audit({ event: "browser.tab_opened", step: step.id });
      break;
    }
    case "browser.close_tab": {
      await current().close();
      const remaining = page.context().pages();
      activePage = remaining[0] ?? (await page.context().newPage());
      await activePage.bringToFront().catch(() => undefined);
      audit({ event: "browser.tab_closed", step: step.id });
      break;
    }
    case "browser.wait_navigation": {
      if (value ?? url) {
        await assertUrl(current(), (value ?? url ?? "") as string, timeout);
      } else {
        await current().waitForLoadState("domcontentloaded", { timeout }).catch(() => undefined);
      }
      break;
    }
    case "ui.fill": {
      const locator = await locate();
      const text = (value ?? "").startsWith("secret://") ? secretValue(value as string) : (value ?? "");
      if ((value ?? "").startsWith("secret://")) audit({ event: "secret.used", step: step.id, ref: value });
      await locator.fill(text, { timeout });
      break;
    }
    case "ui.click":
    case "ui.activate": await (await locate()).click({ timeout }); break;
    case "ui.press": await (await locate()).press((value ?? "Enter") as string, { timeout }); break;
    case "ui.select": await (await locate()).selectOption({ label: value ?? "" }, { timeout }); break;
    case "ui.focus": await (await locate()).focus({ timeout }); break;
    case "ui.scroll": {
      if (!value || !["up", "down", "left", "right"].includes(value)) {
        throw new Error(`step "${step.id}" (ui.scroll) needs direction up, down, left, or right`);
      }
      const renderedIterations = step.iterations !== undefined
        ? substitute(step.iterations, inputs, results)
        : "1";
      const iterations = parseScrollIterations(renderedIterations, step.id);
      await performScroll(
        await locate(),
        current(),
        value as ScrollDirection,
        iterations,
        timeout,
        (event) => audit({ ...event, step: step.id }),
      );
      break;
    }
    case "ui.drag":
    case "ui.drop": {
      const source = await locate();
      if (step.destination) {
        const target = await resolveTarget(current(), step.destination, audit);
        await source.dragTo(target.locator, { timeout });
        audit({ event: "step.drag", step: step.id, strategy: target.resolution.strategy });
      } else if (value && /^-?\d+\s*,\s*-?\d+$/.test(value)) {
        const [dx, dy] = value.split(",").map((part) => Number(part.trim()));
        const box = await source.boundingBox();
        if (!box) throw new Error(`step "${step.id}" (${step.op}) has no visible box to drag`);
        const mouse = current().mouse;
        await mouse.move(box.x + box.width / 2, box.y + box.height / 2);
        await mouse.down();
        await mouse.move(box.x + box.width / 2 + dx, box.y + box.height / 2 + dy, { steps: 8 });
        await mouse.up();
      } else {
        throw new Error(`step "${step.id}" (${step.op}) needs a destination target or an "x,y" offset value`);
      }
      break;
    }
    case "ui.extract": {
      const locator = step.target ? (await resolveTarget(current(), step.target, audit)).locator : current().locator("body");
      const texts = await locator.allInnerTexts();
      const max = limit !== undefined && limit !== "" ? Number(substitute(limit, inputs, results)) : texts.length;
      results[step.id] = texts.slice(0, Number.isFinite(max) ? max : texts.length);
      break;
    }
    case "ui.wait":
    case "ui.find":
      if (step.target) await locate();
      else await current().waitForTimeout(1000);
      break;
    case "app.open": {
      const tab = await page.context().newPage();
      const destination = url ?? value;
      if (destination) {
        if (workflow.policy?.allowed_origins?.length && !originAllowed(destination, workflow.policy.allowed_origins)) {
          throw new Error(`ORIGIN_VIOLATION: ${new URL(destination).origin}`);
        }
        await tab.goto(destination, { waitUntil: "domcontentloaded", timeout });
      }
      await tab.bringToFront();
      activePage = tab;
      audit({ event: "app.opened", step: step.id });
      break;
    }
    case "app.close": {
      await current().close();
      const remaining = page.context().pages();
      activePage = remaining[0] ?? (await page.context().newPage());
      await activePage.bringToFront().catch(() => undefined);
      audit({ event: "app.closed", step: step.id });
      break;
    }
    case "app.focus":
      await current().bringToFront();
      break;
    case "file.choose":
    case "file.upload": {
      const locator = await locate();
      const uploadPath = filePath ?? value;
      if (!uploadPath) throw new Error(`step "${step.id}" (${step.op}) needs a path or value`);
      await locator.setInputFiles(uploadPath, { timeout });
      audit({ event: "file.attached", step: step.id });
      break;
    }
    case "file.download": {
      const downloadPath = filePath ?? value;
      const downloadPromise = current().waitForEvent("download", { timeout }).catch(() => undefined);
      if (step.target) {
        await (await locate()).click({ timeout });
      }
      const download = await downloadPromise;
      if (download && downloadPath) {
        mkdirSync(dirname(downloadPath), { recursive: true });
        await download.saveAs(downloadPath);
        results[step.id] = downloadPath;
        audit({ event: "file.downloaded", step: step.id, path: downloadPath });
      } else if (download) {
        results[step.id] = await download.path().catch(() => undefined) ?? null;
      }
      break;
    }
    case "file.save": {
      if (!filePath && !value) throw new Error(`step "${step.id}" (file.save) needs a path or value`);
      const destination = filePath ?? value ?? "";
      mkdirSync(dirname(destination), { recursive: true });
      writeFileSync(destination, redactObservation(value ?? renderValue(results[step.id] ?? ""), inputs));
      results[step.id] = destination;
      audit({ event: "file.saved", step: step.id, path: destination });
      break;
    }
    case "auth.ensure_session": {
      const body = await current().locator("body").innerText({ timeout: 5000 }).catch(() => "");
      if (!body.trim()) throw new Error(`step "${step.id}" (auth.ensure_session) found an empty session page`);
      audit({ event: "auth.session_ok", step: step.id });
      break;
    }
    case "auth.request_secret": {
      const ref = value ?? "";
      if (!ref.startsWith("secret://")) throw new Error(`step "${step.id}" (auth.request_secret) needs a secret:// value`);
      results[step.id] = secretValue(ref);
      audit({ event: "secret.used", step: step.id, ref });
      break;
    }
    case "auth.require_user":
      if (yes) {
        audit({ event: "step.human", step: step.id, human: "auto-confirmed with --yes" });
        break;
      }
      if (!process.stdin.isTTY) throw new Error(`POLICY_DENIED: step "${step.id}" needs a human`);
      process.stderr.write(`${value ?? step.op} [press Enter] `);
      await new Promise<void>((resolve) => process.stdin.once("data", () => resolve()));
      break;
    case "assert.exists": await locate(); break;
    case "assert.not_exists": {
      if (!step.target) throw new Error(`step "${step.id}" needs a target`);
      const exists = await resolveTarget(current(), step.target, audit, 250).then(() => true).catch(() => false);
      if (exists) throw new Error("POSTCONDITION_FAILED: element still exists");
      break;
    }
    case "assert.url": await assertUrl(current(), (value ?? url ?? "") as string, timeout); break;
    case "assert.text": {
      const body = await current().locator("body").innerText();
      if (!body.includes(value ?? "")) throw new Error("POSTCONDITION_FAILED: text not found");
      break;
    }
    case "assert.schema": {
      const keys = Object.keys(results);
      const actual = keys.length > 0 ? results[keys[keys.length - 1]] : null;
      assertSchemaMatches(value ?? "any", actual, step.id);
      break;
    }
    case "assert.state": {
      if (step.target) await locate();
      const expected = value ?? url ?? "";
      if (expected.startsWith("url:")) {
        await assertUrl(current(), expected.slice(4), timeout);
      } else if (expected) {
        const body = await current().locator("body").innerText();
        const currentUrl = current().url();
        if (!body.includes(expected) && !currentUrl.includes(expected)) {
          throw new Error("POSTCONDITION_FAILED: state not found");
        }
      }
      break;
    }
    case "human.confirm":
    case "human.authenticate":
    case "human.resolve":
      if (yes) {
        audit({ event: "step.human", step: step.id, human: "auto-confirmed with --yes" });
        break;
      }
      if (!process.stdin.isTTY) throw new Error(`POLICY_DENIED: step "${step.id}" needs a human`);
      process.stderr.write(`${value ?? step.op} [press Enter] `);
      await new Promise<void>((resolve) => process.stdin.once("data", () => resolve()));
      break;
    case "control.if":
    case "control.switch": {
      const rendered = step.condition !== undefined ? substitute(step.condition, inputs, results) : "";
      const taken = evalCondition(rendered);
      audit({ event: "control.branch", step: step.id, taken });
      results[step.id] = taken;
      break;
    }
    case "control.loop": {
      const rendered = step.iterations !== undefined ? substitute(step.iterations, inputs, results) : "";
      const count = parseIterations(rendered, step.id);
      audit({ event: "control.loop", step: step.id, iterations: count });
      results[step.id] = count;
      break;
    }
    case "control.retry":
    case "control.timeout":
      audit({ event: "step.note", step: step.id, note: "control marker; recovery policy owns execution" });
      break;
    default:
      throw new Error(`unsupported op in executor v1: ${step.op}`);
  }
  return { resolution, page: activePage };
}

async function main(): Promise<void> {
  const irFile = arg("--ir");
  if (!irFile) {
    console.error("usage: executor.js --ir FILE [--inputs JSON] [--profile DIR] [--headed] [--dry-run] [--yes] [--audit FILE] [--repair-dir DIR] [--state-dir DIR] [--artifact-dir DIR] [--experience-file FILE] [--observe]");
    process.exit(2);
  }
  const raw = readFileSync(irFile, "utf8");
  const workflow = parseIr(raw);
  const inputs = JSON.parse(arg("--inputs") ?? "{}") as Record<string, unknown>;
  const profile = arg("--profile") ?? "";
  const dryRun = flag("--dry-run");
  const yes = flag("--yes");
  const auditFile = arg("--audit") ?? "";
  const repairDir = arg("--repair-dir") ?? "";
  const stateDir = arg("--state-dir") ?? "";
  const artifactDir = arg("--artifact-dir") ?? "";
  const experienceFile = arg("--experience-file") ?? "";
  const observe = flag("--observe") || workflow.recovery?.capture_aria === true;
  const runId = `run_${Date.now()}_${process.pid}`;
  const artifacts: string[] = [];

  if (workflow.policy?.max_runs_per_hour !== undefined) {
    if (!stateDir) {
      console.error("POLICY_DENIED: max_runs_per_hour requires --state-dir");
      process.exit(2);
    }
    mkdirSync(stateDir, { recursive: true });
    const rateFile = join(stateDir, `rate-${workflow.id.replace(/[^A-Za-z0-9_.-]/g, "_")}.json`);
    let timestamps: number[] = [];
    if (existsSync(rateFile)) {
      try {
        const parsed: unknown = JSON.parse(readFileSync(rateFile, "utf8"));
        if (!Array.isArray(parsed) || parsed.some((value) => typeof value !== "number")) {
          throw new Error("invalid rate state");
        }
        timestamps = parsed;
      } catch {
        console.error(`POLICY_DENIED: corrupt rate-limit state ${rateFile}`);
        process.exit(2);
      }
    }
    const window = rateWindow(timestamps, workflow.policy.max_runs_per_hour);
    if (!window.allowed) {
      console.error(`POLICY_DENIED: workflow exceeds ${workflow.policy.max_runs_per_hour} runs per hour`);
      process.exit(2);
    }
    writeFileSync(rateFile, JSON.stringify([...window.active, Date.now()]));
  }

  if (auditFile) mkdirSync(dirname(auditFile), { recursive: true });
  const experienceEvents: Record<string, unknown>[] = [];
  const audit = (event: Record<string, unknown>): void => {
    experienceEvents.push({ ts_ms: Date.now(), ...event });
    if (auditFile) {
      appendFileSync(auditFile, JSON.stringify({ run_id: runId, ts_ms: Date.now(), ...event }) + "\n");
    }
  };

  for (const [name, def] of Object.entries(workflow.inputs ?? {})) {
    if (!(name in inputs)) {
      if (def.default !== undefined) {
        inputs[name] = def.default;
      } else if (def.required !== false) {
        console.error(`missing required input "${name}"`);
        process.exit(2);
      }
    }
  }

  if (workflow.runtime && workflow.runtime !== "browser") {
    const variable = `AGENTMESH_RUNTIME_${workflow.runtime.toUpperCase().replace(/[^A-Z0-9]+/g, "_")}`;
    const adapter = process.env[variable];
    if (!adapter) {
      console.error(`unsupported runtime "${workflow.runtime}"; set ${variable} to an adapter executable`);
      process.exit(3);
    }
    for (const step of workflow.steps) {
      assertOperationAllowed(workflow, step);
      if (WRITE_OPS.has(step.op) && !yes && !dryRun) {
        console.error(`POLICY_DENIED: step "${step.id}" writes; rerun with --yes`);
        process.exit(2);
      }
    }
    audit({ event: "runtime.adapter_started", runtime: workflow.runtime, adapter_variable: variable });
    const child = spawnSync(
      adapter,
      ["--ir", irFile, "--inputs", JSON.stringify(inputs)],
      { encoding: "utf8", maxBuffer: 64 * 1024 * 1024, env: process.env },
    );
    if (child.status !== 0) {
      const detail = String(child.stderr || child.error || "adapter failed").slice(0, 4000);
      const durableDetail = redactObservation(detail, inputs);
      audit({ event: "runtime.adapter_failed", runtime: workflow.runtime, error: durableDetail });
      if (experienceFile) {
        mkdirSync(dirname(experienceFile), { recursive: true });
        appendFileSync(experienceFile, JSON.stringify({
          run_id: runId,
          ts_ms: Date.now(),
          workflow: workflow.id,
          runtime: workflow.runtime,
          status: "failed",
          input_names: Object.keys(inputs).sort(),
          error: durableDetail,
          trajectory: experienceEvents,
        }) + "\n");
      }
      console.error(detail);
      process.exit(3);
    }
    const last = String(child.stdout).trim().split("\n").filter(Boolean).pop() ?? "{}";
    let result: Record<string, unknown>;
    try {
      result = JSON.parse(last) as Record<string, unknown>;
    } catch {
      result = { outputs: last };
    }
    audit({ event: "runtime.adapter_completed", runtime: workflow.runtime });
    if (experienceFile) {
      mkdirSync(dirname(experienceFile), { recursive: true });
      appendFileSync(experienceFile, JSON.stringify({
        run_id: runId,
        ts_ms: Date.now(),
        workflow: workflow.id,
        runtime: workflow.runtime,
        status: "succeeded",
        input_names: Object.keys(inputs).sort(),
        completed_steps: workflow.steps.length,
        artifacts: result.artifacts ?? [],
        trajectory: experienceEvents,
      }) + "\n");
    }
    process.stdout.write(JSON.stringify({ run_id: runId, outputs: result.outputs ?? result, artifacts: result.artifacts ?? [] }) + "\n");
    return;
  }

  const results: Record<string, unknown> = {};
  let context: BrowserContext;
  let browser: Browser | undefined;
  try {
    if (profile) {
      context = await chromium.launchPersistentContext(profile, {
          channel: "chrome",
          headless: !flag("--headed"),
          args: ["--no-first-run", "--no-default-browser-check"],
        });
    } else {
      browser = await chromium.launch({ channel: "chrome", headless: !flag("--headed") });
      context = await browser.newContext();
    }
  } catch (error) {
    console.error(`cannot start Chrome: ${String(error).split("\n")[0]}`);
    console.error("Install Google Chrome or pass --profile with a prepared session.");
    process.exit(3);
  }
  const initialPage = await firstPage(context);
  let currentPage = initialPage;
  audit({ event: "run.started", workflow: workflow.id });

  const confirmWrite = (step: IrStep): void => {
    if (yes || dryRun) return;
    if (!process.stdin.isTTY) {
      throw new Error(`POLICY_DENIED: step "${step.id}" writes; rerun with --yes`);
    }
    process.stderr.write(`Step "${step.id}" (${step.op}) performs a write. Continue? [y/N] `);
    const answer = (process.stdin.read() as Buffer | null)?.toString().trim().toLowerCase();
    if (answer !== "y" && answer !== "yes") {
      throw new Error("POLICY_DENIED: write not confirmed");
    }
  };

  try {
    const total = workflow.steps.length;
    let done = 0;
    let preconditionsChecked = false;
    let checkpointUrl = "";
    if (!workflow.steps.some((step) => step.op === "browser.navigate")) {
      await requireAssertions("precondition", workflow.preconditions ?? [], currentPage, inputs, results, audit);
      preconditionsChecked = true;
    }
    const runOne = async (step: IrStep): Promise<Resolution | undefined> => {
      assertOperationAllowed(workflow, step);
      audit({ event: "step.started", step: step.id, op: step.op });
      process.stderr.write(`[${done + 1}/${total}] ${step.op} ${step.id}\n`);
      if (WRITE_OPS.has(step.op)) {
        confirmWrite(step);
      }
      if (observe) {
        const path = await captureAria(currentPage, artifactDir, `${done + 1}-${step.id}-before`, inputs, audit);
        if (path) artifacts.push(path);
      }
      const maxAttempts = workflow.recovery?.max_attempts ?? 1;
      let resolution: Resolution | undefined;
      for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
        try {
          const outcome = await executeStep(currentPage, workflow, step, inputs, results, yes, audit);
          if (outcome.page) currentPage = outcome.page;
          resolution = outcome.resolution;
          break;
        } catch (error) {
          audit({
            event: "step.attempt_failed",
            step: step.id,
            attempt,
            retryable: retryable(error),
            error: String(error instanceof Error ? error.message : error),
          });
          if (attempt >= maxAttempts || !retryable(error)) throw error;
          if (checkpointUrl && currentPage.url() !== checkpointUrl) {
            await currentPage.goto(checkpointUrl, { waitUntil: "domcontentloaded" });
            audit({ event: "recovery.rollback", step: step.id, checkpoint_url: checkpointUrl });
          }
          await currentPage.waitForTimeout(Math.min(250 * attempt, 1000));
        }
      }

      if (
        workflow.policy?.allowed_origins?.length &&
        !originAllowed(currentPage.url(), workflow.policy.allowed_origins)
      ) {
        throw new Error(`ORIGIN_VIOLATION: redirected to ${currentPage.url()}`);
      }

      for (const detector of workflow.failure ?? []) {
        if (await assertionMatches(currentPage, detector, inputs, results, audit, false)) {
          audit({ event: "failure.detected", step: step.id, op: detector.op });
          throw new Error(`WORKFLOW_FAILURE_DETECTED: ${detector.op}`);
        }
      }

      if ((workflow.recovery?.checkpoints ?? []).includes(step.id)) {
        checkpointUrl = currentPage.url();
        audit({ event: "checkpoint.saved", step: step.id, url: checkpointUrl });
      }

      if (observe) {
        const path = await captureAria(currentPage, artifactDir, `${done + 1}-${step.id}-after`, inputs, audit);
        if (path) artifacts.push(path);
      }

      if (resolution && resolution.strategy !== "selector" && repairDir && step.target?.selectors?.length) {
        mkdirSync(repairDir, { recursive: true });
        writeFileSync(
          join(repairDir, `${workflow.id}.${step.id}.json`),
          JSON.stringify({
            workflow: workflow.id,
            step: step.id,
            old_selector: step.target.selectors[0],
            resolved_via: resolution.strategy,
            resolved_as: resolution.describe,
            confidence: resolution.confidence,
            ts_ms: Date.now(),
          }, null, 2),
        );
        audit({ event: "repair.proposed", step: step.id, strategy: resolution.strategy });
      }
      audit({ event: "step.completed", step: step.id });
      done += 1;
      process.stderr.write(`✓ [${done}/${total}] ${step.id}\n`);
      return resolution;
    };
    for (let index = 0; index < workflow.steps.length; index += 1) {
      const step = workflow.steps[index];
      if (dryRun && !READ_OPS.has(step.op)) {
        audit({ event: "step.completed", step: step.id, skipped: "dry-run stops before writes" });
        continue;
      }
      if ((step.op === "control.if" || step.op === "control.switch") && index + 1 < workflow.steps.length) {
        await runOne(step);
        const taken = results[step.id] === true || (results[step.id] !== false && evalCondition(String(results[step.id] ?? "")));
        if (!taken) {
          audit({ event: "control.skipped", step: workflow.steps[index + 1].id, guard: step.id });
          index += 1;
        }
        continue;
      }
      if (step.op === "control.loop" && index + 1 < workflow.steps.length) {
        await runOne(step);
        const count = typeof results[step.id] === "number" ? results[step.id] as number : 1;
        const body = workflow.steps[index + 1];
        for (let round = 0; round < (count as number); round += 1) {
          if (dryRun && !READ_OPS.has(body.op)) {
            audit({ event: "step.completed", step: body.id, skipped: "dry-run stops before writes" });
            continue;
          }
          audit({ event: "control.iteration", step: body.id, round: round + 1, of: count });
          await runOne({ ...body, id: `${body.id}#${round + 1}` });
        }
        if (!preconditionsChecked && body.op === "browser.navigate") {
          await requireAssertions("precondition", workflow.preconditions ?? [], currentPage, inputs, results, audit);
          preconditionsChecked = true;
        }
        index += 1;
        continue;
      }
      await runOne(step);

      if (!preconditionsChecked && step.op === "browser.navigate") {
        await requireAssertions("precondition", workflow.preconditions ?? [], currentPage, inputs, results, audit);
        preconditionsChecked = true;
      }
    }
    await requireAssertions("success", workflow.success ?? [], currentPage, inputs, results, audit);
    process.stderr.write(`✓ ${workflow.id}: ${done}/${total} steps\n`);

    const outputs: Record<string, unknown> = {};
    for (const [name, def] of Object.entries(workflow.outputs ?? {})) {
      const match = /^steps\.([A-Za-z0-9_]+)(?:\.result)?$/.exec(def.from);
      outputs[name] = match ? (results[match[1]] ?? null) : null;
    }
    audit({ event: "run.completed", artifacts });
    if (experienceFile) {
      mkdirSync(dirname(experienceFile), { recursive: true });
      appendFileSync(experienceFile, JSON.stringify({
        run_id: runId,
        ts_ms: Date.now(),
        workflow: workflow.id,
        status: "succeeded",
        input_names: Object.keys(inputs).sort(),
        completed_steps: done,
        artifacts,
        trajectory: experienceEvents,
      }) + "\n");
    }
    await context.close();
    await browser?.close();
    process.stdout.write(JSON.stringify({ run_id: runId, outputs, artifacts }) + "\n");
  } catch (error) {
    const message = String(error instanceof Error ? error.message : error);
    if (artifactDir && workflow.recovery?.capture_screenshot !== false) {
      try {
        mkdirSync(artifactDir, { recursive: true });
        const screenshot = join(artifactDir, "failure.png");
        await currentPage.screenshot({ path: screenshot, fullPage: true });
        artifacts.push(screenshot);
        audit({ event: "observation.screenshot", path: screenshot });
        if (workflow.recovery?.vision_adapter) {
          const request = join(artifactDir, "vision-repair-request.json");
          writeFileSync(request, JSON.stringify({
            workflow: workflow.id,
            adapter: workflow.recovery.vision_adapter,
            screenshot,
            error: message,
            instruction: "Propose a semantic target repair; never execute coordinates without review.",
          }, null, 2));
          artifacts.push(request);
          audit({ event: "vision.repair_requested", adapter: workflow.recovery.vision_adapter, path: request });
          const adapterKey = `AGENTMESH_VISION_ADAPTER_${workflow.recovery.vision_adapter.toUpperCase().replace(/[^A-Z0-9]+/g, "_")}`;
          const adapter = process.env[adapterKey] ?? process.env.AGENTMESH_VISION_ADAPTER;
          if (adapter) {
            const vision = spawnSync(adapter, [request], {
              encoding: "utf8",
              maxBuffer: 16 * 1024 * 1024,
              env: process.env,
            });
            if (vision.status === 0 && String(vision.stdout).trim()) {
              const response = join(artifactDir, "vision-repair-response.json");
              writeFileSync(response, String(vision.stdout));
              artifacts.push(response);
              audit({ event: "vision.repair_proposed", adapter: workflow.recovery.vision_adapter, path: response });
            } else {
              audit({
                event: "vision.adapter_failed",
                adapter: workflow.recovery.vision_adapter,
                error: String(vision.stderr || vision.error || "adapter failed").slice(0, 2000),
              });
            }
          }
        }
      } catch (captureError) {
        audit({ event: "observation.failed", label: "failure-screenshot", error: String(captureError) });
      }
    }
    audit({ event: "run.failed", error: message, artifacts });
    if (experienceFile) {
      mkdirSync(dirname(experienceFile), { recursive: true });
      appendFileSync(experienceFile, JSON.stringify({
        run_id: runId,
        ts_ms: Date.now(),
        workflow: workflow.id,
        status: "failed",
        input_names: Object.keys(inputs).sort(),
        error: message,
        artifacts,
        trajectory: experienceEvents,
      }) + "\n");
    }
    await context.close().catch(() => undefined);
    await browser?.close().catch(() => undefined);
    console.error(message);
    process.exit(message.includes("POLICY_DENIED") ? 2 : 3);
  }
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
  void main();
}
