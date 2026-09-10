// Called by report-browser-tests.mjs: real offline, static and hydrated pages.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

// Observe the actual renderer result without replacing its rendering or events.
const observeCharts = `
window.reportCharts = [];
Object.defineProperty(window, "vegaEmbed", {
  configurable: true,
  get() { return this.reportEmbed; },
  set(embed) {
    this.reportEmbed = (...args) => embed(...args).then(result => {
      window.reportCharts.push({ view: result.view, spec: args[1] });
      return result;
    });
  }
});`;

function horizontalGeometry() {
  return Array.from(document.querySelectorAll(".value-scroll"), region => {
    const card = region.closest(".card").getBoundingClientRect();
    const box = region.getBoundingClientRect();
    return {
      name: region.getAttribute("aria-label"),
      contained: box.left >= card.left && box.right <= card.right,
      overflow: getComputedStyle(region).overflowX,
      keyboard: region.tabIndex === 0 && region.getAttribute("role") === "region",
      slots: region.closest(".card").querySelectorAll('[data-role="value"]').length,
    };
  });
}

export async function testReportLayout({ temporary, openReport }) {
  const source = join(temporary, "layout.gcl");
  writeFileSync(source, readFileSync(new URL("./fixtures/report-layout.gcl", import.meta.url)));
  for (const interactive of [false, true]) {
    const output = join(temporary, `layout-${interactive}.html`);
    const markdown = join(temporary, `layout-${interactive}.md`);
    const args = ["report", "build", source, "--output", output, "--markdown", markdown, ...interactive ? [] : ["--static"]];
    function build() {
      const result = spawnSync("target/debug/graphcal", args, { encoding: "utf8", timeout: 30000 });
      assert.equal(result.status, 0, result.stderr);
    }
    build();
    const baseline = [readFileSync(output), readFileSync(markdown)];
    build();
    assert.deepEqual([readFileSync(output), readFileSync(markdown)], baseline, "byte-deterministic HTML and Markdown");
    const { command, evaluate, wait, warnings, exceptions, close } = await openReport(output, observeCharts);
    await wait("window.reportCharts?.length > 0");
    if (interactive) await wait("document.querySelector('.hydration-status')?.textContent === 'live'");
    const region = name => `document.querySelector('[data-decl="${name}"] .value-scroll')`;
    async function checkLayout() {
      for (const width of [1440, 390, 280]) {
        await command("Emulation.setDeviceMetricsOverride", { width, height: 1000, deviceScaleFactor: 1, mobile: false });
        const geometry = await evaluate(`(${horizontalGeometry})()`);
        assert.equal(geometry.length, 8);
        for (const card of geometry) {
          assert.equal(card.contained, true, `${width}px: ${card.name}`);
          assert.equal(card.overflow, "auto");
          assert.equal(card.keyboard, true);
          assert.equal(card.slots, 1);
        }
        assert.equal(await evaluate("document.documentElement.scrollWidth <= innerWidth"), true, `${width}px page must not overflow`);
        assert.equal(await evaluate(`${region("samples")}.querySelectorAll('tbody tr').length`), 64);
        assert.equal(await evaluate(`${region("slices")}.querySelectorAll('td').length`), 128);
        assert.equal(await evaluate(`${region("wide")}.scrollWidth > ${region("wide")}.clientWidth`), true);
        // A real keyboard event can reveal the far-right columns locally.
        await evaluate(`${region("wide")}.focus(); ${region("wide")}.scrollLeft = 0`);
        await command("Input.dispatchKeyEvent", { type: "keyDown", key: "ArrowRight", code: "ArrowRight", windowsVirtualKeyCode: 39 });
        await command("Input.dispatchKeyEvent", { type: "keyUp", key: "ArrowRight", code: "ArrowRight", windowsVirtualKeyCode: 39 });
        await wait(`${region("wide")}.scrollLeft > 0`);
        // Let the browser's keyboard-scroll animation finish before resizing.
        await evaluate("new Promise(resolve => setTimeout(resolve, 250))");
      }
    }
    async function checkZoom() {
      await command("Emulation.setDeviceMetricsOverride", { width: 1440, height: 1000, deviceScaleFactor: 1, mobile: false });
      const before = await evaluate(`(() => {
        const chart = window.reportCharts.at(-1);
        window.zoomView = chart.view;
        return { x: chart.view.scale('x').domain(), y: chart.view.scale('y').domain(), encodings: chart.spec.params[0].select.encodings };
      })()`);
      assert.deepEqual(before.encodings, ["y"]);
      const point = await evaluate(`(() => {
        const canvas = document.querySelector('figure canvas');
        canvas.scrollIntoView({ block: 'center' });
        const box = canvas.getBoundingClientRect();
        return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
      })()`);
      await command("Input.dispatchMouseEvent", { type: "mouseMoved", ...point });
      await command("Input.dispatchMouseEvent", { type: "mouseWheel", ...point, deltaX: 0, deltaY: -800 });
      await wait(`JSON.stringify(window.zoomView.scale('y').domain()) !== ${JSON.stringify(JSON.stringify(before.y))}`);
      assert.deepEqual(await evaluate("window.zoomView.scale('x').domain()"), before.x, "categorical domain must not zoom");
    }
    await checkLayout();
    await checkZoom();
    if (interactive) {
      await evaluate(`(() => {
        const field = document.querySelector('[data-decl="gain"] .control-field');
        field.value = '3.0'; field.dispatchEvent(new Event('input', { bubbles: true }));
        window.savedRegion = ${region("wide")};
        window.savedRegion.focus(); window.savedRegion.scrollLeft = 100;
      })()`);
      await wait("document.querySelector('[data-decl=\"total\"] [data-role=\"value\"]').textContent === '37.5'");
      assert.deepEqual(await evaluate(`({ focused: document.activeElement === window.savedRegion, same: window.savedRegion === ${region("wide")}, left: window.savedRegion.scrollLeft })`), { focused: true, same: true, left: 100 }, "recalculation preserves focus and scroll position");
      await checkLayout();
      await checkZoom();
      await evaluate("document.querySelector('.modified-banner__reset').click()");
      await wait("document.querySelector('[data-decl=\"total\"] [data-role=\"value\"]').textContent === '25'");
      await checkLayout();
    }
    assert.deepEqual(warnings, [], "valid charts should not emit renderer warnings");
    assert.deepEqual(exceptions, [], "no uncaught browser exceptions");
    await close();
  }
  console.log("Chrome: static/hydrated table containment, keyboard scrolling, redraw focus, determinism and categorical/continuous zoom passed");
}
