import { test } from "node:test";
import assert from "node:assert/strict";
import { describeEvent } from "../dist/recorder.js";

test("navigate line carries the url", () => {
  assert.equal(
    describeEvent({ kind: "browser.navigate", url: "https://app.example/login" }),
    "browser.navigate https://app.example/login",
  );
});

test("fill line carries tag, role, name, selector and literal", () => {
  const line = describeEvent({
    kind: "ui.fill",
    url: "https://app.example/",
    target: { tag: "input", role: "textbox", name: "Search", selectors: ["#q"] },
    value: { literal: "beta" },
  });
  assert.match(line, /^ui\.fill /);
  assert.match(line, /textbox/);
  assert.match(line, /"beta"/);
});

test("secret fill logs the ref, never a value", () => {
  const line = describeEvent({
    kind: "ui.fill",
    url: "https://app.example/",
    target: { tag: "input", role: "textbox", name: "Password", selectors: ["#pw"] },
    value: { secret_ref: "secret://app/Password" },
  });
  assert.match(line, /secret:\/\/app\/Password/);
  assert.doesNotMatch(line, /s3cret/);
});

test("fill line shows the placeholder when the field has no name", () => {
  const line = describeEvent({
    kind: "ui.fill",
    url: "https://app.example/",
    target: { tag: "input", role: "textbox", placeholder: "Teléfono", selectors: ["#t"] },
    value: { literal: "809" },
  });
  assert.match(line, /placeholder="Teléfono"/);
});

test("click line carries the button text", () => {
  const line = describeEvent({
    kind: "ui.click",
    url: "https://app.example/",
    target: { tag: "button", role: "button", text: "Go", selectors: ["#go"] },
  });
  assert.match(line, /"Go"/);
});

test("submit line carries the field for Enter-to-submit", () => {
  const line = describeEvent({
    kind: "ui.submit",
    url: "https://app.example/login",
    target: { tag: "input", role: "textbox", name: "Password", selectors: ["#pw"] },
  });
  assert.match(line, /^ui\.submit /);
  assert.match(line, /textbox/);
});

test("scroll line carries direction and infinite-pagination marker", () => {
  const line = describeEvent({
    kind: "ui.scroll",
    target: { tag: "div", role: "feed", name: "Messages", selectors: ["#feed"] },
    deltaX: 0,
    deltaY: 240,
    untilStable: true,
  });
  assert.match(line, /^ui\.scroll /);
  assert.match(line, /down until_stable$/);
});

test("session boundaries log without payloads", () => {
  assert.equal(
    describeEvent({ kind: "session.start", scope: "https://app.example", headed: false }),
    "session.start scope=https://app.example",
  );
  assert.equal(describeEvent({ kind: "session.stop" }), "session.stop");
});
