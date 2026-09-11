// Real file:// report hydration in an isolated headless Chrome profile.
// Usage: GRAPHCAL_CHROME=/path/to/chrome node internals/report-browser-tests.mjs
// CDP: https://chromedevtools.github.io/devtools-protocol/
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { once } from "node:events";
import { testReportLayout } from "./report-layout-browser-tests.mjs";
import { setTimeout as delay } from "node:timers/promises";

const temporary = mkdtempSync(join(tmpdir(), "graphcal-browser-"));
const chrome = process.env.GRAPHCAL_CHROME || "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const child = spawn(chrome, ["--headless", "--remote-debugging-port=0", `--user-data-dir=${join(temporary, "profile")}`, "about:blank"], { stdio: ["ignore", "ignore", "pipe"] });
let socket;
const exit = once(child, "exit");
const watchdog = setTimeout(() => child.kill("SIGKILL"), 180000);
try {
  const endpoint = await new Promise((resolve, reject) => {
    child.once("error", reject);
    let log = "";
    child.stderr.on("data", chunk => {
      log += chunk;
      const found = log.match(/DevTools listening on (ws:\/\/[^\s]+)/);
      if (found) resolve(new URL(found[1]));
    });
    child.once("exit", code => reject(new Error(`Chrome exited: ${code}\n${log}`)));
  });
  async function openReport(output, initScript = "") {
    const target = await (await fetch(`http://${endpoint.host}/json/new?about:blank`, { method: "PUT" })).json();
    socket = new WebSocket(target.webSocketDebuggerUrl);
    await once(socket, "open");
    const connection = socket;
    const warnings = [];
    const exceptions = [];
    let id = 0;
    const pending = new Map();
    socket.addEventListener("message", event => {
      const message = JSON.parse(event.data);
      if (message.method === "Runtime.consoleAPICalled" && message.params.type === "warning") warnings.push(message.params.args);
      if (message.method === "Runtime.exceptionThrown") exceptions.push(message.params.exceptionDetails);
      if (pending.has(message.id)) {
        const { resolve, reject } = pending.get(message.id);
        pending.delete(message.id);
        if (message.error) reject(new Error(JSON.stringify(message.error))); else resolve(message.result);
      }
    });
    const command = (method, params = {}) => new Promise((resolve, reject) => {
      pending.set(++id, { resolve, reject });
      connection.send(JSON.stringify({ id, method, params }));
    });
    const evaluate = async expression => {
      const result = await command("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true, timeout: 25000 });
      assert.ok(!result.exceptionDetails, JSON.stringify(result.exceptionDetails));
      return result.result.value;
    };
    const wait = async expression => {
      for (let attempt = 0; attempt < 200; attempt++) {
        if (await evaluate(`Boolean(${expression})`)) return;
        await delay(100);
      }
      throw new Error(`Timed out: ${expression}\n${await evaluate("document.body.innerText")}`);
    };
    await command("Runtime.enable");
    await command("Page.enable");
    await command("Network.enable");
    await command("Network.setBlockedURLs", { urls: ["http://*", "https://*"] });
    if (initScript) await command("Page.addScriptToEvaluateOnNewDocument", { source: initScript });
    await command("Page.navigate", { url: pathToFileURL(output).href });
    return { command, evaluate, wait, warnings, exceptions, close: async () => {
      connection.close();
      socket = null;
      await fetch(`http://${endpoint.host}/json/close/${target.id}`);
    } };
  }
  for (const initial of [0, 1]) {
    const model = `param divisor: Dimensionless = ${initial}.0;
plot curve = { mark: line, encode: { x: for i: Fin(2) { 1.0 }, y: for i: Fin(2) { 1.0 / @divisor } } };
${initial ? "plot healthy = { mark: point, encode: { x: 1.0, y: 2.0 } };" : ""}`;
    const source = join(temporary, "main.gcl");
    const output = join(temporary, `report-${initial}.html`);
    writeFileSync(source, model);
    const built = spawnSync("target/debug/graphcal", ["report", "build", source, "--output", output], { encoding: "utf8", timeout: 30000 });
    assert.equal(built.status, initial ? 0 : 1, built.stderr);
    const { evaluate, wait, close } = await openReport(output);
    const draft = async value => evaluate(`(() => { const field = document.querySelector('[data-decl="divisor"] .control-field'); field.value = ${JSON.stringify(value)}; field.dispatchEvent(new Event('input', { bubbles: true })); })()`);
    const apply = async () => evaluate(`document.querySelector('[data-decl="divisor"] .control-apply').click()`);
    const edit = async value => { await draft(value); await apply(); };
    const chart = `document.querySelector('figure[data-figure="curve"] canvas')`;
    const failure = `document.querySelector('figure[data-figure="curve"] .error-chip')`;
    await wait(`document.querySelector('.hydration-status')?.textContent.startsWith('live')`);
    if (!initial) {
      await wait(`${failure}?.textContent.includes('division by zero')`);
      assert.equal(await evaluate("typeof window.vegaEmbed"), "function", "failed baseline must include renderer assets");
      await draft("1.0");
      await delay(300);
      assert.equal(await evaluate(`Boolean(${chart})`), false, "unapplied edits must retain the failed baseline");
      await apply();
    }
    await wait(chart);
    await edit("0.0");
    await wait(`${failure}?.textContent.includes('division by zero')`);
    assert.equal(await evaluate(`Boolean(${chart})`), false, "obsolete data must be removed");
    if (initial) await wait(`document.querySelector('figure[data-figure="healthy"] canvas')`);
    await edit("2.0");
    await wait(chart);
    assert.equal(await evaluate(`Boolean(${failure})`), false);

    // Missing targets are reconstructed, including their figure caption.
    await evaluate(`document.querySelector('figure[data-figure="curve"]').remove()`);
    await edit("3.0");
    await wait(chart);
    await evaluate("window.savedEmbed = window.vegaEmbed; window.vegaEmbed = undefined");
    await edit("4.0");
    await wait(`${failure}?.textContent.includes('renderer is unavailable')`);
    assert.equal(await evaluate(`Boolean(${chart})`), false);
    await evaluate("window.vegaEmbed = () => Promise.reject(new Error('injected renderer rejection'))");
    await edit("5.0");
    await wait(`${failure}?.textContent.includes('injected renderer rejection')`);
    await evaluate("window.vegaEmbed = window.savedEmbed");
    await edit("6.0");
    await wait(chart);

    // A renderer resolving after a newer failure may only touch its detached mount.
    await evaluate("window.vegaEmbed = (...args) => new Promise(resolve => setTimeout(() => resolve(window.savedEmbed(...args)), 800))");
    await edit("7.0");
    await delay(350);
    await edit("0.0");
    await wait(`${failure}?.textContent.includes('division by zero')`);
    await delay(1000);
    assert.equal(await evaluate(`Boolean(${chart})`), false);
    await evaluate("window.vegaEmbed = window.savedEmbed; document.querySelector('.modified-banner__reset').click()");
    await wait(initial ? chart : `${failure}?.textContent.includes('division by zero')`);
    await close();
  }
  {
    const source = join(temporary, "structured.gcl");
    const output = join(temporary, "structured.html");
    writeFileSync(source, `pub type Choice { Amount(value: Length), Off, }
param choice: Choice = Amount(value: 2.0 m);
param samples: Int[Fin(40)] = for i: Fin(40) { 1 };`);
    const built = spawnSync("target/debug/graphcal", ["report", "build", source, "--output", output], { encoding: "utf8", timeout: 30000 });
    assert.equal(built.status, 0, built.stderr);
    const { evaluate, wait, exceptions, close } = await openReport(output);
    await wait(`document.querySelector('.hydration-status')?.textContent.startsWith('live')`);
    assert.equal(await evaluate(`document.querySelectorAll('[data-decl="samples"] .control-index-entry').length`), 32);
    await evaluate(`document.querySelector('[data-decl="samples"] .control-more').click()`);
    assert.equal(await evaluate(`document.querySelectorAll('[data-decl="samples"] .control-index-entry').length`), 40);
    await evaluate(`(() => {
      const field = document.querySelector('[data-decl="choice"] .control-field');
      field.value = '3.0 s'; field.dispatchEvent(new Event('input', { bubbles: true }));
      document.querySelector('[data-decl="choice"] .control-apply').click();
    })()`);
    await wait(`document.querySelector('[data-decl="choice"] .control-error--nested')`);
    assert.equal(await evaluate(`document.querySelector('[data-decl="choice"] [data-role="value"]').textContent.includes('2 m')`), true, "invalid nested input retains the accepted result");
    await evaluate(`(() => {
      const field = document.querySelector('[data-decl="choice"] .control-field');
      field.value = '3.0 m'; field.dispatchEvent(new Event('input', { bubbles: true }));
      document.querySelector('[data-decl="choice"] .control-apply').click();
    })()`);
    await wait(`document.querySelector('[data-decl="choice"] [data-role="value"]').textContent.includes('3 m')`);
    const draft = await evaluate(`document.querySelector('[data-decl="choice"] .control-field').value`);
    await evaluate(`(() => {
      const select = document.querySelector('[data-decl="choice"] .control-constructor');
      select.value = '1'; select.dispatchEvent(new Event('change', { bubbles: true }));
      document.querySelector('[data-decl="choice"] .control-apply').click();
    })()`);
    await wait(`document.querySelector('[data-decl="choice"] [data-role="value"]').textContent.trim() === 'Off'`);
    await evaluate(`(() => {
      const select = document.querySelector('[data-decl="choice"] .control-constructor');
      select.value = '0'; select.dispatchEvent(new Event('change', { bubbles: true }));
    })()`);
    assert.equal(await evaluate(`document.querySelector('[data-decl="choice"] .control-field').value`), draft, "constructor-specific drafts survive toggles");
    assert.deepEqual(exceptions, [], "structured controls must not throw browser exceptions");
    await close();
  }
  await testReportLayout({ temporary, openReport });
  console.log("Chrome: initial failure, success/failure/recovery, mixed plots, missing targets/assets, renderer rejection, late completion and reset passed");
} finally {
  if (socket) socket.close();
  child.kill();
  await exit;
  clearTimeout(watchdog);
  rmSync(temporary, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
}
