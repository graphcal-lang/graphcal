import { test, expect, type Page } from "@playwright/test";
import { gzipSync, gunzipSync } from "node:zlib";
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

test("report controls update results and share exact applied bindings and focused view", async ({
  page,
  context,
}) => {
  await open(page);
  await expect(result(page)).toHaveText("4 m/s");
  await expect(report(page).locator("figure canvas, figure svg")).toBeVisible();
  await expect(report(page).locator(".card-doc")).toHaveText("Speed <script> is plain text.");
  await report(page).getByRole("textbox", { name: "speed", exact: true }).fill("36.0 km/h");
  await expect(result(page)).toHaveText("20 m/s");
  await expect(page.locator("#output")).toContainText("20 m/s");
  await expect(report(page).locator(".repro")).toContainText("speed=36.0 km/h");
  await report(page).getByRole("checkbox", { name: "enabled", exact: true }).uncheck();
  await expect(result(page, "enabled")).toHaveText("false");
  await report(page)
    .getByRole("combobox", { name: "mode", exact: true })
    .selectOption("Mode#Nominal");
  await expect(result(page, "mode")).toContainText("Nominal");
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

test("rejected and pending edits do not replace or share successful values", async ({ page }) => {
  await open(page, [{ name: "speed", expr: "7.0 m/s" }]);
  const field = report(page).getByRole("textbox", { name: "speed", exact: true });
  await field.fill("3.0 kg");
  await expect(
    report(page)
      .locator(".control-error")
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
  expect((await share(page)).state.bindings).toEqual([{ name: "speed", expr: "7.0 m/s" }]);
  await page.clock.runFor(400);
  await expect(result(page)).toHaveText("18 m/s");
  await field.fill("11.0 m/s");
  await field.fill("13.0 m/s");
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
  await expect.poll(state).toBe(true);
  await page.clock.install();
  await page.clock.pauseAt(new Date(Date.now() + 1000));
  await field.fill("13.0 m/s");
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
  await expect(result(page)).toHaveText("10 m/s");
  expect((await share(page)).state.bindings).toEqual([{ name: "speed", expr: "5.0 m/s" }]);
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
