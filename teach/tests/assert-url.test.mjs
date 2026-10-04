import { test } from "node:test";
import assert from "node:assert/strict";
import { assertUrl } from "../dist/executor.js";

test("assertUrl returns once the URL contains the fragment", async () => {
  let calls = 0;
  const page = { url: () => (++calls >= 3 ? "https://app.example/quotes" : "https://app.example/") };
  await assertUrl(page, "/quotes", 5000);
  assert.ok(calls >= 3);
});

test("assertUrl fails closed when the URL never arrives", async () => {
  const page = { url: () => "https://app.example/" };
  await assert.rejects(assertUrl(page, "/quotes", 600), /POSTCONDITION_FAILED: url .* lacks \/quotes/);
});
