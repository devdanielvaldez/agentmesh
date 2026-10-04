import { test } from "node:test";
import assert from "node:assert/strict";
import { resolveTarget } from "../dist/executor.js";

const visible = { first: () => ({ waitFor: async () => undefined }) };
const missing = {
  first: () => ({
    waitFor: async () => {
      throw new Error("locator.waitFor: Timeout 5000ms exceeded.");
    },
  }),
};
const stubPage = ({ role = missing, text = missing, label = missing, css = () => missing } = {}) => ({
  locator: (...args) => css(...args),
  getByRole: () => role,
  getByText: () => text,
  getByLabel: () => label,
  getByPlaceholder: () => missing,
});

test("bare role still resolves when nothing stronger was recorded", async () => {
  const events = [];
  const found = await resolveTarget(
    stubPage({ role: visible }),
    { role: "button" },
    (e) => events.push(e),
  );
  assert.equal(found.resolution.strategy, "role");
  assert.ok(events.every((e) => e.event !== "target.blind_match_refused"));
});

test("recorded text resolves by text before a bare role", async () => {
  const r = await resolveTarget(
    stubPage({ text: visible }),
    { role: "button", text: "Go" },
    () => undefined,
  );
  assert.equal(r.resolution.strategy, "text");
});

test("stale selector falls back to autocomplete for rotating login ids", async () => {
  const css = (selector) => (String(selector).includes("autocomplete") ? visible : missing);
  const r = await resolveTarget(
    stubPage({ css }),
    { role: "textbox", selectors: ["#_r_3_"], autocomplete: "username" },
    () => undefined,
  );
  assert.equal(r.resolution.strategy, "autocomplete");
});

test("recorded text that matches nothing fails closed instead of blind-clicking", async () => {
  const events = [];
  await assert.rejects(
    resolveTarget(stubPage(), { role: "button", text: "Missing" }, (e) => events.push(e)),
    /TARGET_NOT_FOUND/,
  );
  assert.ok(events.some((e) => e.event === "target.blind_match_refused"));
});
