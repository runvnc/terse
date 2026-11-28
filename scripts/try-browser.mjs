#!/usr/bin/env node
// Drives the website's try page (try.html) in a headless Chrome, never a
// window: opens <url>, presses Start (or uses the page's #<program>),
// waits for frames, presses keys, takes screenshots and prints what the
// page measured. Node 22 or later (its WebSocket speaks Chrome's DevTools
// protocol); no packages.
//
//     node scripts/try-browser.mjs <chrome> <url> <out dir> [step ...]
//
// Steps: `start` (click Start), `wait <frames>` (machine frames),
// `press <code>` (a key by KeyboardEvent.code, e.g. ArrowDown, Enter, KeyA),
// `type <text>`, `shot <name.png>`, `click <x> <y>` (screen units), `home`
// (the page's Home button).
// Prints `frames <n> frame-ms <machines> draw-ms <drawing>` at the end and
// `error <text>` for any error the page logged; exits 1 on one.

import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const [chrome, url, out, ...rest] = process.argv.slice(2);
if (!chrome || !url || !out) {
  console.error("usage: try-browser.mjs <chrome> <url> <out dir> [step ...]");
  process.exit(2);
}
const profile = mkdtempSync(join(tmpdir(), "terse-try-chrome-"));
const proc = spawn(chrome, [
  "--headless=new", "--no-first-run", "--no-default-browser-check", "--disable-extensions",
  "--disable-background-networking", "--disable-sync", "--mute-audio", "--hide-scrollbars",
  `--user-data-dir=${profile}`, "--remote-debugging-port=0", "--window-size=1100,1000", "about:blank",
], { stdio: ["ignore", "ignore", "pipe"] });
let failed = false;
const done = (code) => {
  try { proc.kill("SIGKILL"); } catch {}
  try { rmSync(profile, { recursive: true, force: true }); } catch {}
  process.exit(code);
};
setTimeout(() => { console.log("error timeout"); done(1); }, 120000);

const portFile = join(profile, "DevToolsActivePort");
for (let i = 0; i < 300 && !existsSync(portFile); i++) await new Promise((r) => setTimeout(r, 100));
const port = readFileSync(portFile, "utf8").split("\n")[0];
const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
const page = targets.find((t) => t.type === "page");
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener("open", r));
let id = 0;
const pending = new Map();
ws.addEventListener("message", (m) => {
  const msg = JSON.parse(m.data);
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg);
    pending.delete(msg.id);
  } else if (msg.method === "Runtime.exceptionThrown") {
    console.log("error " + JSON.stringify(msg.params.exceptionDetails.exception?.description || msg.params.exceptionDetails.text));
    failed = true;
  } else if (msg.method === "Runtime.consoleAPICalled" && msg.params.type === "error") {
    console.log("error " + msg.params.args.map((a) => a.value ?? a.description).join(" "));
    failed = true;
  }
});
const send = (method, params = {}) => new Promise((resolve, reject) => {
  const n = ++id;
  pending.set(n, (msg) => (msg.error ? reject(new Error(method + ": " + msg.error.message)) : resolve(msg.result)));
  ws.send(JSON.stringify({ id: n, method, params }));
});
const evaluate = async (expr) => (await send("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: true })).result.value;

await send("Runtime.enable");
await send("Page.enable");
await send("Emulation.setDeviceMetricsOverride", { width: 1100, height: 1000, deviceScaleFactor: 1, mobile: false });
await send("Page.navigate", { url });
for (let i = 0; i < 100 && (await evaluate("document.readyState")) !== "complete"; i++) await new Promise((r) => setTimeout(r, 100));
await evaluate("document.querySelector('.terse-demo').scrollIntoView()");

const frames = async () => Number(await evaluate("(document.querySelector('.terse-demo')||{dataset:{}}).dataset.frames || 0"));
async function waitFrames(n) {
  const until = (await frames()) + n;
  for (let i = 0; i < 600; i++) {
    if ((await frames()) >= until) return;
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(`no ${n} frames`);
}
async function key(code) {
  const keys = {
    Enter: ["Enter", 13, "\r"], Escape: ["Escape", 27], Tab: ["Tab", 9], Space: [" ", 32, " "], Backspace: ["Backspace", 8],
    ArrowDown: ["ArrowDown", 40], ArrowUp: ["ArrowUp", 38], ArrowLeft: ["ArrowLeft", 37], ArrowRight: ["ArrowRight", 39],
    F12: ["F12", 123], PageDown: ["PageDown", 34], PageUp: ["PageUp", 33],
  };
  let [k, vk, text] = keys[code] || [];
  if (!k && code.startsWith("Key")) { k = code.slice(3).toLowerCase(); vk = k.toUpperCase().charCodeAt(0); text = k; }
  if (!k && code.startsWith("Digit")) { k = code.slice(5); vk = k.charCodeAt(0); text = k; }
  await send("Input.dispatchKeyEvent", { type: text ? "keyDown" : "rawKeyDown", key: k, code, windowsVirtualKeyCode: vk, text });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code, windowsVirtualKeyCode: vk });
}
async function screenPoint(x, y) {
  const r = await evaluate(`(() => { const r = document.querySelector('.terse-demo canvas').getBoundingClientRect(); return [r.left, r.top, r.width, r.height]; })()`);
  const s = Math.min(r[2] / 640, r[3] / 480);
  return [r[0] + (r[2] - 640 * s) / 2 + (x + 0.5) * s, r[1] + (r[3] - 480 * s) / 2 + (y + 0.5) * s];
}

try {
  const steps = [...rest];
  while (steps.length) {
    const s = steps.shift();
    if (s === "start") {
      await evaluate("document.querySelector('.terse-demo [data-start=\"\"]').click()");
      for (let i = 0; i < 300 && (await frames()) === 0; i++) await new Promise((r) => setTimeout(r, 100));
      if ((await frames()) === 0) throw new Error("no frame after Start: " + (await evaluate("document.querySelector('.status').textContent")));
    } else if (s === "home") {
      await evaluate("document.querySelector('.terse-demo .home').click()");
      await waitFrames(2);
    } else if (s === "wait") await waitFrames(Number(steps.shift()));
    else if (s === "press") { await key(steps.shift()); await waitFrames(2); }
    else if (s === "type") { for (const c of steps.shift()) { await send("Input.insertText", { text: c }); } await waitFrames(2); }
    else if (s === "click") {
      const [x, y] = await screenPoint(Number(steps.shift()), Number(steps.shift()));
      for (const type of ["mousePressed", "mouseReleased"]) await send("Input.dispatchMouseEvent", { type, x, y, button: "left", buttons: type === "mousePressed" ? 1 : 0, clickCount: 1 });
      await waitFrames(2);
    } else if (s === "shot") {
      const shot = await send("Page.captureScreenshot", { format: "png" });
      writeFileSync(join(out, steps.shift()), Buffer.from(shot.data, "base64"));
    } else throw new Error("unknown step " + s);
  }
  const d = await evaluate("JSON.stringify(document.querySelector('.terse-demo').dataset)");
  const ds = JSON.parse(d || "{}");
  console.log(`frames ${ds.frames || 0} frame-ms ${ds.frameMs || "-"} draw-ms ${ds.drawMs || "-"}`);
  console.log("status " + (await evaluate("document.querySelector('.terse-demo .status').textContent")));
} catch (e) {
  console.log("error " + e.message);
  failed = true;
}
done(failed ? 1 : 0);
