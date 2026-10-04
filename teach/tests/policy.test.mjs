import test from "node:test";
import assert from "node:assert/strict";

import {
  assertOperationAllowed,
  operationMatches,
  originAllowed,
  rateWindow,
  redactObservation,
} from "../dist/executor.js";

test("operation policy supports exact operations and family wildcards", () => {
  assert.equal(operationMatches("ui.click", "ui.click"), true);
  assert.equal(operationMatches("ui.*", "ui.fill"), true);
  assert.equal(operationMatches("ui.*", "browser.navigate"), false);
});

test("rate window drops old starts and fails closed at the limit", () => {
  const now = 10_000_000;
  assert.deepEqual(rateWindow([now - 3_700_000, now - 10], 2, now), {
    allowed: true,
    active: [now - 10],
  });
  assert.equal(rateWindow([now - 20, now - 10], 2, now).allowed, false);
});

test("origin policy compares scheme, host and port", () => {
  assert.equal(originAllowed("https://app.example/path", ["https://app.example"]), true);
  assert.equal(originAllowed("http://app.example/path", ["https://app.example"]), false);
  assert.equal(originAllowed("https://app.example:8443", ["https://app.example"]), false);
  assert.equal(originAllowed("about:blank", ["https://app.example"]), true);
});

test("denied operations take precedence over allowed operations", () => {
  const workflow = {
    policy: {
      allowed_operations: ["ui.*"],
      denied_operations: ["ui.click"],
    },
  };
  assert.throws(
    () => assertOperationAllowed(workflow, { id: "delete", op: "ui.click" }),
    /POLICY_DENIED/,
  );
  assert.doesNotThrow(() =>
    assertOperationAllowed(workflow, { id: "type", op: "ui.fill" }),
  );
  assert.throws(
    () => assertOperationAllowed(workflow, { id: "open", op: "browser.navigate" }),
    /not allowed/,
  );
});

test("durable observations redact caller-provided values", () => {
  const snapshot = '- textbox "Search": private-query\n- text: private-query result on page 12';
  const redacted = redactObservation(snapshot, { query: "private-query", page: 12 });
  assert.equal(redacted.includes("private-query"), false);
  assert.equal(redacted.includes("{{ input.query }}"), true);
  assert.equal(redacted.includes("{{ input.page }}"), true);
});
