import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  assertSchemaMatches,
  evalCondition,
  parseIterations,
  substitute,
} from "../dist/executor.js";

describe("steps.* substitution", () => {
  it("resolves earlier step results alongside inputs", () => {
    const out = substitute("read {{ steps.login.result }} as {{ inputs.user }}", { user: "ana" }, { login: ["a", "b"] });
    assert.equal(out, 'read ["a","b"] as ana');
  });

  it("fails closed on unknown step results", () => {
    assert.throws(() => substitute("{{ steps.ghost }}", {}, {}), /missing step result/);
  });
});

describe("control helpers", () => {
  it("treats empty/false/zero as falsy", () => {
    for (const falsy of ["", "false", "FALSE", "0", "no", "null", "undefined", "none", "  "]) {
      assert.equal(evalCondition(falsy), false, JSON.stringify(falsy));
    }
    for (const truthy of ["yes", "1", "found:alpha", "true"]) {
      assert.equal(evalCondition(truthy), true, JSON.stringify(truthy));
    }
  });

  it("bounds loop iterations to 1-100", () => {
    assert.equal(parseIterations("3", "repeat"), 3);
    assert.throws(() => parseIterations("0", "repeat"), /iterations 1-100/);
    assert.throws(() => parseIterations("101", "repeat"), /iterations 1-100/);
    assert.throws(() => parseIterations("many", "repeat"), /iterations 1-100/);
  });
});

describe("assert.schema", () => {
  it("accepts matching types and rejects mismatches", () => {
    assertSchemaMatches("array", ["a"], "check");
    assertSchemaMatches('{"type":"array","minItems":1}', ["a"], "check");
    assert.throws(() => assertSchemaMatches("array", "nope", "check"), /POSTCONDITION_FAILED/);
    assert.throws(
      () => assertSchemaMatches('{"type":"array","minItems":2}', ["a"], "check"),
      /at least 2 items/,
    );
  });
});
