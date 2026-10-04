import test from "node:test";
import assert from "node:assert/strict";
import { RECORDER_INIT_SCRIPT } from "../dist/recorder.js";

test("recorder init script installs debounced semantic wheel capture", () => {
  assert.doesNotThrow(() => new Function(RECORDER_INIT_SCRIPT));
  assert.match(RECORDER_INIT_SCRIPT, /addEventListener\("wheel"/);
  assert.match(RECORDER_INIT_SCRIPT, /scrollRegion\(start, deltaX, deltaY\)/);
  assert.match(RECORDER_INIT_SCRIPT, /emit\("ui\.scroll"/);
  assert.match(RECORDER_INIT_SCRIPT, /untilStable: pending\.untilStable/);
});
