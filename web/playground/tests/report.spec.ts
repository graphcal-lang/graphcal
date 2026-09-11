import { test, expect, type Page } from "@playwright/test";
import { gzipSync, gunzipSync } from "node:zlib";
import { readFileSync } from "node:fs";
import AxeBuilder from "@axe-core/playwright";

const source = `/// Speed <script> is plain text.
param speed: Velocity = 2.0 m/s;
param trials: Int(min: 1, max: 10) = 3;
param enabled: Bool = true;
pub index Mode = { Nominal, Safe };
param mode: Key<Mode> = Mode#Safe;
node doubled: Velocity = @speed * 2.0;
assert positive = @speed > 0.0 m/s;
plot curve = { mark: point, encode: { x: 1.0, y: @speed / (1.0 m/s) } };
`;
const fragment = (bindings: { name: string; expr: string }[] = [], text = source) =>
  `#v=2&code=${gzipSync(JSON.stringify({ document: { filename: "report.gcl", source: text }, bindings })).toString("base64url")}`;
const report = (page: Page) => page.frameLocator("#report iframe");
const result = (page: Page, name = "doubled") =>
  report(page).locator(`[data-decl="${name}"] [data-role="value"]`);
async function apply(page: Page, name: string) {
  const menu = report(page)
    .locator(`.outline-row[data-parameter="${name}"] .outline-row-actions`)
    .first();
  await menu.locator("summary").click();
  await menu.getByRole("button", { name: "Apply", exact: true }).click();
}
async function open(page: Page, bindings: { name: string; expr: string }[] = []) {
  await page.goto(`/playground/?view=report${fragment(bindings)}`);
  await expect(page.locator("#report")).toContainText("Run to generate");
  await expect(page.locator("#report iframe")).toHaveCount(0);
  await page.locator("#run").click();
  await expect(page.locator("#status")).toHaveText("Up to date");
  await expect(report(page).getByRole("textbox", { name: "speed", exact: true })).toBeVisible();
}
async function share(page: Page) {
  await page.locator("#share").click();
  await expect(page.locator("#share-link")).toBeVisible();
  const link = await page.locator("#share-link").inputValue();
  const encoded = new URLSearchParams(new URL(link).hash.slice(1)).get("code")!;
  return { link, state: JSON.parse(gunzipSync(Buffer.from(encoded, "base64url")).toString()) };
}

test("report controls auto-run, support manual Apply, and share accepted bindings", async ({
  page,
  context,
}) => {
  await open(page);
  await expect(result(page)).toHaveText("4 m/s");
  await report(page).getByRole("button", { name: "Plots", exact: true }).click();
  await expect(report(page).locator("figure canvas, figure svg")).toBeVisible();
  await report(page).getByRole("button", { name: "Values", exact: true }).click();
  await expect(report(page).locator(".card-doc")).toHaveText("Speed <script> is plain text.");
  const autoRun = report(page).getByRole("checkbox", { name: "Auto run", exact: true });
  const speed = report(page).getByRole("textbox", { name: "speed", exact: true });
  await expect(autoRun).toBeChecked();
  await speed.fill("36.0 km/h");
  await expect(result(page)).toHaveText("20 m/s");
  await expect(page.locator("#output")).toContainText("20 m/s");
  await expect(report(page).locator(".repro")).toContainText("speed=36.0 km/h");
  await report(page).getByRole("checkbox", { name: "enabled", exact: true }).uncheck();
  await expect(result(page, "enabled")).toHaveText("false");
  await report(page)
    .getByRole("combobox", { name: "mode", exact: true })
    .selectOption("Mode#Nominal");
  await expect(result(page, "mode")).toContainText("Nominal");
  await autoRun.uncheck();
  await speed.fill("5.0 m/s");
  await page.waitForTimeout(700);
  await expect(result(page)).toHaveText("20 m/s");
  await apply(page, "speed");
  await expect(result(page)).toHaveText("10 m/s");
  await autoRun.check();
  await speed.fill("36.0 km/h");
  await expect(result(page)).toHaveText("20 m/s");
  const { link, state } = await share(page);
  expect(new URL(link).searchParams.get("view")).toBe("report");
  expect(state.document.source).toBe(source);
  expect(state.bindings).toContainEqual({ name: "speed", expr: "36.0 km/h" });
  const fresh = await context.newPage();
  await fresh.goto(link);
  await expect(fresh.locator("#status")).toContainText("press Run");
  await expect(fresh.locator("#workspace")).toBeHidden();
  await fresh.locator("#run").click();
  await expect(result(fresh)).toHaveText("20 m/s");
  await expect(report(fresh).getByRole("textbox", { name: "speed", exact: true })).toHaveValue(
    "36.0 km/h",
  );
  await expect(result(fresh, "enabled")).toHaveText("false");
  await expect(result(fresh, "mode")).toContainText("Nominal");
  await fresh.locator("#reset-parameters").click();
  await expect(result(fresh)).toHaveText("4 m/s");
  await expect(result(fresh, "enabled")).toHaveText("true");
  await fresh.close();
});

test("recursive structured controls apply atomically and paginate fixed axes", async ({ page }) => {
  const structured = `pub type Choice { Amount(value: Length), Off, }
param choice: Choice = Amount(value: 2.0 m);
param samples: Int[Fin(40)] = for i: Fin(40) { 1 };`;
  await page.goto(`/playground/?view=report${fragment([], structured)}`);
  await page.locator("#run").click();
  const frame = report(page);
  await expect(frame.locator(".hydration-status")).toHaveText("live");
  await expect(frame.locator('[data-decl="samples"] .control-index-entry')).toHaveCount(32);
  await frame.getByRole("searchbox", { name: "Search inputs" }).fill("samples #39");
  await expect(frame.locator('[data-decl="samples"] .control-index-entry')).toHaveCount(40);
  await frame.getByRole("textbox", { name: "samples #39", exact: true }).fill("7");
  await expect(result(page, "samples")).toContainText("7");
  await frame.getByRole("searchbox", { name: "Search inputs" }).fill("");

  const amount = frame.getByRole("textbox", { name: "choice value", exact: true });
  await amount.fill("3.0 s");
  await expect(result(page, "choice")).toContainText("2 m");
  await apply(page, "choice");
  await expect(
    frame
      .locator('.outline-row[data-parameter="choice"] .outline-error')
      .filter({ hasText: /dimension|unit|type/i }),
  ).toBeVisible();
  await expect(result(page, "choice")).toContainText("2 m");
  await amount.fill("3.0 m");
  await apply(page, "choice");
  await expect(result(page, "choice")).toContainText("3 m");

  const constructor = frame.getByRole("combobox", { name: "choice constructor", exact: true });
  await constructor.selectOption({ label: "Off" });
  await apply(page, "choice");
  await expect(result(page, "choice")).toHaveText("Off");
  await constructor.selectOption({ label: "Amount" });
  await expect(frame.getByRole("textbox", { name: "choice value", exact: true })).toHaveValue(
    "3.0 m",
  );
});

test("report result tabs stay inside the sandbox and survive recalculation", async ({ page }) => {
  await open(page);
  for (const speed of ["3.0 m/s", "4.0 m/s"]) {
    await report(page).getByRole("textbox", { name: "speed", exact: true }).fill(speed);
    await apply(page, "speed");
    await expect(result(page)).toHaveText(speed === "3.0 m/s" ? "6 m/s" : "8 m/s");
    const tab = report(page).getByRole("button", { name: "Plots", exact: true });
    await tab.focus();
    await page.keyboard.press("Enter");
    await expect(tab).toHaveAttribute("aria-pressed", "true");
    await expect(report(page).locator("figure canvas, figure svg")).toBeVisible();
    await expect(page.locator("#report iframe")).toHaveCount(1);
    expect(page.frames().some((frame) => frame.url() === "about:srcdoc")).toBe(true);
  }
});

test("rejected and pending edits do not replace or share successful values", async ({ page }) => {
  await open(page, [{ name: "speed", expr: "7.0 m/s" }]);
  const field = report(page).getByRole("textbox", { name: "speed", exact: true });
  await field.fill("3.0 kg");
  await apply(page, "speed");
  await expect(
    report(page)
      .locator(".outline-error")
      .filter({ hasText: /dimension|unit|type/i }),
  ).toBeVisible();
  await expect(result(page)).toHaveText("14 m/s");
  const rejected = await share(page);
  expect(rejected.state.bindings).toEqual([{ name: "speed", expr: "7.0 m/s" }]);
  await expect(page.locator("#share-status")).toContainText("excluded");
  // Freeze debounce, not the WASM worker, to exercise Share before evaluation.
  await page.clock.install();
  await page.clock.pauseAt(new Date(Date.now() + 1000));
  await field.fill("9.0 m/s");
  await apply(page, "speed");
  expect((await share(page)).state.bindings).toEqual([{ name: "speed", expr: "7.0 m/s" }]);
  await page.clock.runFor(400);
  await expect(result(page)).toHaveText("18 m/s");
  await field.fill("11.0 m/s");
  await field.fill("13.0 m/s");
  await apply(page, "speed");
  await page.clock.runFor(400);
  await expect(result(page)).toHaveText("26 m/s");
});

test("view history preserves dirty source and source changes clear parameter overrides", async ({
  page,
}) => {
  await open(page, [{ name: "speed", expr: "8.0 m/s" }]);
  await page.locator("#workspace-view").click();
  await page.getByRole("textbox", { name: "Graphcal source editor" }).fill(source + "\n// edited");
  await page.locator("#report-view").click();
  await expect(page.locator("#report")).toContainText("overrides cleared");
  page.on("dialog", () => {
    throw new Error("view-only navigation must not prompt");
  });
  await page.goBack();
  await expect(page.locator("#workspace")).toBeVisible();
  await expect(page.getByRole("textbox", { name: "Graphcal source editor" })).toContainText(
    "edited",
  );
  await page.goForward();
  await expect(page.locator("#report")).toBeVisible();
  await page.locator("#run").click();
  await expect(result(page)).toHaveText("4 m/s");
  expect((await share(page)).state.bindings).toEqual([]);
});

test("late evaluations cannot replace newer edits, and Stop cancels debounced controls", async ({
  page,
}) => {
  await page.addInitScript(() => {
    const state = { hold: false, held: false, release: () => {} };
    Object.assign(window, { reportTestWorker: state });
    const NativeWorker = window.Worker;
    window.Worker = class extends NativeWorker {
      constructor(url: string | URL, options?: WorkerOptions) {
        super(url, options);
        this.addEventListener("message", (event) => {
          if (state.hold && event.data.kind === "result") {
            state.hold = false;
            state.held = true;
            state.release = () =>
              this.dispatchEvent(new MessageEvent("message", { data: event.data }));
            event.stopImmediatePropagation();
          }
        });
      }
    };
  });
  await open(page);
  const state = () =>
    page.evaluate(
      () => (window as unknown as { reportTestWorker: { held: boolean } }).reportTestWorker.held,
    );
  await page.evaluate(() => {
    (window as unknown as { reportTestWorker: { hold: boolean } }).reportTestWorker.hold = true;
  });
  const field = report(page).getByRole("textbox", { name: "speed", exact: true });
  await field.fill("11.0 m/s");
  await apply(page, "speed");
  await expect.poll(state).toBe(true);
  await page.clock.install();
  await page.clock.pauseAt(new Date(Date.now() + 1000));
  await field.fill("13.0 m/s");
  await apply(page, "speed");
  await expect(page.locator("#status")).toContainText("Parameters changed");
  await page.evaluate(() => {
    (window as unknown as { reportTestWorker: { release: () => void } }).reportTestWorker.release();
  });
  // The rejected late reply must not become either the visible or shared result.
  expect((await share(page)).state.bindings).toEqual([]);
  await expect(result(page)).toHaveText("4 m/s");
  await expect(page.locator("#stop")).toBeEnabled();
  await page.clock.runFor(400);
  await expect(result(page)).toHaveText("26 m/s");
  await field.fill("19.0 m/s");
  await apply(page, "speed");
  await expect(page.locator("#stop")).toBeEnabled();
  await page.locator("#stop").click();
  await page.clock.runFor(400);
  await expect(page.locator("#report iframe")).toHaveCount(0);
  await expect(page.locator("#status")).toHaveText("Stopped");
  await page.locator("#run").click();
  await expect(result(page)).toHaveText("26 m/s");
});

test("invalid restored overrides are explicit and recover with Reset parameters", async ({
  page,
}) => {
  await page.goto(`/playground/?view=report${fragment([{ name: "missing", expr: "3.0" }])}`);
  await page.locator("#run").click();
  await expect(page.locator("#status")).toContainText("missing");
  await expect(page.locator("#report iframe")).toHaveCount(0);
  await page.locator("#reset-parameters").click();
  await expect(result(page)).toHaveText("4 m/s");
});

test("opaque iframe denies parent access, forged messages and external plot resources", async ({
  page,
}) => {
  await open(page);
  const frame = page.frames().find((frame) => frame.url() === "about:srcdoc")!;
  expect(
    await frame.evaluate(() => {
      try {
        return parent.document.title;
      } catch {
        return "denied";
      }
    }),
  ).toBe("denied");
  await page.evaluate(() =>
    window.postMessage(
      {
        type: "evaluate",
        session: "forged",
        id: 1,
        bindings: [{ name: "speed", expr: "99.0 m/s" }],
      },
      "*",
    ),
  );
  await frame.evaluate(() =>
    parent.postMessage(
      {
        type: "evaluate",
        session: "forged",
        id: 1,
        bindings: [{ name: "speed", expr: "99.0 m/s" }],
      },
      "*",
    ),
  );
  await expect(result(page)).toHaveText("4 m/s");
  expect(
    await frame.evaluate(async () => {
      try {
        await fetch("https://example.com/private");
        return "allowed";
      } catch {
        return "denied";
      }
    }),
  ).toBe("denied");
  await report(page).getByRole("textbox", { name: "speed", exact: true }).fill("5.0 m/s");
  await apply(page, "speed");
  await expect(result(page)).toHaveText("10 m/s");
  expect((await share(page)).state.bindings).toEqual([{ name: "speed", expr: "5.0 m/s" }]);
});

test("structured report tables are contained, bounded and accessible", async ({ page }) => {
  const layoutSource = readFileSync(
    new URL("../../../crates/graphcal-report/tests/fixtures/report-layout.gcl", import.meta.url),
    "utf8",
  );
  await page.goto(`/playground/?view=report${fragment([], layoutSource)}`);
  await page.locator("#run").click();
  await expect(report(page).locator(".hydration-status")).toHaveText("live");
  expect(
    (await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze())
      .violations,
  ).toEqual([]);
  await report(page)
    .locator(".workspace-output-details")
    .evaluateAll((details) => {
      details.forEach((detail) => {
        (detail as HTMLDetailsElement).open = true;
      });
    });
  for (const width of [1440, 390]) {
    await page.setViewportSize({ width, height: 1000 });
    const samples = report(page).getByRole("region", { name: "samples values", exact: true });
    await expect(samples.locator("tbody tr")).toHaveCount(64);
    expect(
      await samples.evaluate(
        (region) => region.clientHeight <= 384 && region.scrollHeight > region.clientHeight,
      ),
    ).toBe(true);
    expect(
      await report(page)
        .locator(".value-scroll")
        .evaluateAll((regions) =>
          regions.every((region) => {
            const card = region.closest(".card")!.getBoundingClientRect();
            const box = region.getBoundingClientRect();
            return (
              box.left >= card.left &&
              box.right <= card.right &&
              region.getAttribute("tabindex") === "0"
            );
          }),
        ),
    ).toBe(true);
  }
});

test("outline search and editable pins share the native evaluator in the sandbox", async ({
  page,
}) => {
  await open(page);
  const frame = report(page);
  await frame.getByRole("searchbox", { name: "Search inputs" }).fill("plain text");
  await expect(frame.locator(".outline-content .outline-row")).toHaveCount(1);
  await frame.getByRole("button", { name: "Pin input speed", exact: true }).click();
  await frame.getByRole("searchbox", { name: "Search inputs" }).fill("");
  await frame.getByRole("checkbox", { name: "Pinned only", exact: true }).check();
  await expect(frame.locator(".outline-content")).toBeHidden();
  await frame.getByRole("button", { name: "Pin output doubled", exact: true }).click();
  await frame.getByRole("button", { name: "Checks", exact: true }).click();
  await frame.getByRole("textbox", { name: "speed", exact: true }).fill("5.0 m/s");
  await expect(result(page)).toHaveText("10 m/s");
  await expect(result(page)).toBeVisible();
  expect((await share(page)).state.bindings).toEqual([{ name: "speed", expr: "5.0 m/s" }]);
  await frame.getByRole("textbox", { name: "speed", exact: true }).fill("5.0 kg");
  await expect(frame.locator(".outline-favorites .outline-error")).toBeVisible();
  await expect(result(page)).toHaveText("10 m/s");
  await frame.getByRole("textbox", { name: "speed", exact: true }).fill("6.0 m/s");
  await expect(result(page)).toHaveText("12 m/s");
});

test.describe("mobile report", () => {
  test.use({ hasTouch: true, viewport: { width: 390, height: 844 } });

  test("readable controls, local navigation and scrolling survive edits and resizing", async ({
    page,
  }) => {
    const layoutSource = readFileSync(
      new URL("../../../crates/graphcal-report/tests/fixtures/report-layout.gcl", import.meta.url),
      "utf8",
    );
    await page.goto(`/playground/?view=report${fragment([], layoutSource)}`);
    await page.locator("#run").tap();
    const frame = report(page);
    await expect(frame.locator(".hydration-status")).toHaveText("live");
    const jump = frame.getByRole("navigation", { name: "Report workspace", exact: true });
    const gain = frame.getByRole("textbox", { name: "gain", exact: true });
    await frame.getByRole("checkbox", { name: "Auto run", exact: true }).uncheck();
    await gain.fill("3.0");

    for (const [width, height] of [
      [390, 844],
      [320, 568],
      [667, 375],
      [390, 400],
    ]) {
      await page.setViewportSize({ width, height });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
        true,
      );
      expect(
        await page.locator("#report iframe").evaluate((el) => el.clientHeight),
      ).toBeGreaterThanOrEqual(height - 1);
      expect(
        await gain.evaluate((el) => {
          const box = el.getBoundingClientRect();
          const row = el.closest(".outline-row")!.getBoundingClientRect();
          return (
            parseFloat(getComputedStyle(el).fontSize) >= 16 &&
            box.height >= 44 &&
            box.width >= row.width - 1
          );
        }),
      ).toBe(true);
      await jump.getByRole("button", { name: "Results", exact: true }).tap();
      await expect(frame.locator("#workspace-results")).toBeFocused();
      expect(
        await frame
          .locator("#workspace-results")
          .evaluate(
            (el) =>
              el.getBoundingClientRect().top >=
              document.querySelector(".workspace-mobile-nav")!.getBoundingClientRect().bottom,
          ),
      ).toBe(true);
      await frame.getByRole("button", { name: "Plots", exact: true }).tap();
      const plot = frame.getByRole("figure", { name: "bars plot" });
      await expect(plot.locator("canvas, svg")).toBeVisible();
      expect(
        await plot.evaluate(
          (el) =>
            el.scrollWidth > el.clientWidth &&
            el.tabIndex === 0 &&
            document.documentElement.scrollWidth <= innerWidth,
        ),
      ).toBe(true);
      expect(
        await frame
          .locator(".workspace-result-body")
          .evaluate((el) => getComputedStyle(el).overflowY),
      ).toBe("visible");
      await jump.getByRole("button", { name: "Inputs", exact: true }).tap();
      await expect(frame.locator("#inputs")).toBeFocused();
      await expect(gain).toHaveValue("3.0");
      await expect(frame.getByRole("button", { name: "Plots", exact: true })).toHaveAttribute(
        "aria-pressed",
        "true",
      );
    }
    // The jump controls must never navigate srcdoc to the host URL or replace the report.
    await expect(page.locator("#report iframe")).toHaveCount(1);
    expect(page.frames().some((child) => child.url() === "about:srcdoc")).toBe(true);
    await apply(page, "gain");
    await frame.getByRole("button", { name: "Values", exact: true }).tap();
    await expect(result(page, "total")).toHaveText("37.5");
    await page.setViewportSize({ width: 1440, height: 1000 });
    await expect(jump).toBeHidden();
    expect(
      await frame.locator(".outline-explorer").evaluate((el) => getComputedStyle(el).overflowY),
    ).toBe("auto");
    await expect(gain).toHaveValue("3.0");
    await page.setViewportSize({ width: 390, height: 844 });
    expect(
      (await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze())
        .violations,
    ).toEqual([]);
  });
});

test("report layout and controls remain accessible at desktop and narrow widths", async ({
  page,
}) => {
  await open(page);
  expect(
    (await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze())
      .violations,
  ).toEqual([]);
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(report(page).getByRole("textbox", { name: "speed", exact: true })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});
