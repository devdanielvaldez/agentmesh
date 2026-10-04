/**
 * AgentMesh Teach session login.
 *
 * Opens the application in a persistent Chromium profile so the user can
 * authenticate once (password, QR, SSO, passkey — AgentMesh never sees it).
 * Later runs reuse the stored session without touching credentials.
 *
 * Usage:
 *   node dist/session-login.js --profile DIR --url URL
 */

import { chromium } from "playwright-core";

function arg(name: string): string | undefined {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
}

async function main(): Promise<void> {
  const profile = arg("--profile");
  const url = arg("--url");
  if (!profile || !url) {
    console.error("usage: session-login.js --profile DIR --url URL");
    process.exit(2);
  }
  const context = await chromium.launchPersistentContext(profile, {
    channel: "chrome",
    headless: false,
    args: ["--no-first-run", "--no-default-browser-check"],
  });
  const page = context.pages()[0] ?? (await context.newPage());
  await page.goto(url, { waitUntil: "domcontentloaded" });
  console.error(`Log in inside the browser window. Press Enter here when done.`);
  await new Promise<void>((resolve) => {
    process.stdin.resume();
    process.stdin.once("data", () => resolve());
  });
  await context.close();
  console.error(`Session stored in ${profile}`);
}

void main().catch((error) => {
  console.error(`session login failed: ${String(error)}`);
  process.exit(1);
});
