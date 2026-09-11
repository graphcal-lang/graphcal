import { test, expect, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { gzipSync } from "node:zlib";
import { readFile } from "node:fs/promises";
import catalog from "../examples/catalog.json" with { type: "json" };
import { decodeFragment } from "../src/share-codec";

function fragment(source: string, filename = "main.gcl") {
  return `#v=1&code=${gzipSync(JSON.stringify({ filename, source })).toString("base64url")}`;
}
async function ready(page: Page) {
  await page.goto("/playground/");
  await expect(page.locator("#status")).toHaveText("Up to date");
}
async function source(page: Page, text: string) {
  await page.getByLabel("Auto-run", { exact: true }).uncheck();
  await page.getByRole("textbox", { name: "Graphcal source editor" }).fill(text);
}

test("full-height editor, keyboard resizing, no tab trap, theme and accessibility", async ({
  page,
}) => {
  await ready(page);
  await expect(page.locator("#output")).toContainText("delta_v");
  const editor = page.locator("#editor");
  expect((await editor.boundingBox())!.height).toBeGreaterThan(400);
  expect((await editor.boundingBox())!.width).toBeGreaterThan(600);
  await page.locator("#separator").focus();
  await page.keyboard.press("ArrowLeft");
  await expect(page.locator("#separator")).toHaveAttribute("aria-valuenow", "55");
  await page.getByRole("textbox", { name: "Graphcal source editor" }).focus();
  await page.keyboard.press("Tab");
  await expect(page.getByRole("textbox", { name: "Graphcal source editor" })).not.toBeFocused();
  expect(
    (await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze())
      .violations,
  ).toEqual([]);
  await page.getByRole("button", { name: "Toggle theme" }).click();
  expect(
    (await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze())
      .violations,
  ).toEqual([]);
  await page.screenshot({ path: "test-results/playground-desktop.png" });
});

for (const example of catalog) {
  test(`catalog example ${example.id} evaluates in a real module worker; plots stay local`, async ({
    page,
    context,
    browserName,
  }) => {
    test.slow(browserName === "webkit", "Trace snapshots of large indexed tables are expensive");
    const requests: string[] = [];
    const errors: string[] = [];
    context.on("request", (request) => {
      requests.push(request.url());
      expect(request.method()).toBe("GET");
      expect(request.postData()).toBeNull();
    });
    page.on("pageerror", (error) => errors.push(error.message));
    await ready(page);
    expect(requests.some((url) => url.includes("/vega/"))).toBe(false);
    await page.getByLabel("Examples", { exact: true }).selectOption(example.id);
    await page.getByRole("button", { name: "Load example", exact: true }).click();
    await expect(page.getByLabel("Filename", { exact: true })).toHaveValue(example.filename);
    await expect(page.locator("#example-description")).toHaveText(example.description);
    await expect(page.locator("#status")).toHaveText("Up to date");
    for (const name of example.expected_values)
      await expect(page.locator("#output")).toContainText(name);
    await expect(page.locator("#output figure")).toHaveCount(example.expected_figures);
    const charts = page.locator("#output figure svg, #output figure canvas");
    await expect(charts).toHaveCount(example.expected_figures);
    for (let i = 0; i < example.expected_figures; i++) await expect(charts.nth(i)).toBeVisible();
    expect(requests.every((url) => new URL(url).origin === "http://127.0.0.1:4173")).toBe(true);
    expect(errors).toEqual([]);
  });
}

test("renders multidimensional indexed values as two-dimensional table slices", async ({
  page,
}) => {
  await ready(page);
  const fixture = await readFile(
    new URL("../../../tests/fixtures/valid/table_literal.gcl", import.meta.url),
    "utf8",
  );
  await source(page, fixture);
  await page.getByRole("button", { name: "Run", exact: true }).click();
  await expect(page.locator("#status")).toHaveText("Up to date");

  const matrix = page.locator('[data-declaration-name="spacecraft_mass"]');
  await matrix.locator("summary").click();
  const matrixTable = matrix.locator("table.value-table");
  await expect(matrixTable).toHaveCount(1);
  await expect(matrixTable.locator('thead th[scope="col"]:not(:first-child)')).toContainText([
    "Departure",
    "Correction",
    "Insertion",
  ]);
  await expect(matrixTable.locator('tbody th[scope="row"]')).toContainText([
    "Launch",
    "Cruise",
    "Arrival",
  ]);
  await expect(matrix.locator("details details")).toHaveCount(0);

  const cube = page.locator('[data-declaration-name="mass_3d"]');
  await cube.locator("summary").click();
  await expect(cube.locator("table.value-table")).toHaveCount(2);
  await expect(cube.locator(".table-selector")).toContainText(["[Nominal]", "[Contingency]"]);
  await expect(cube.locator("details details")).toHaveCount(0);
});

test("shares the edited filename/source and restores without auto-execution", async ({
  page,
  browser,
}) => {
  await ready(page);
  const text = "// 🙂 % # &\nnode shared_value: Length = 3.0 m;\n";
  await source(page, text);
  await page.getByLabel("Filename", { exact: true }).fill("snippet.gcl");
  await page.getByRole("button", { name: "Share", exact: true }).click();
  const link = page.getByRole("textbox", { name: "Shareable URL" });
  await expect(link).toBeVisible();
  const url = await link.inputValue();
  expect(await decodeFragment(new URL(url).hash)).toEqual({
    document: { filename: "snippet.gcl", source: text },
    bindings: [],
  });
  const context = await browser.newContext();
  const fresh = await context.newPage();
  const requests: string[] = [];
  context.on("request", (request) => requests.push(request.url()));
  await fresh.goto(url);
  await expect(fresh.locator("#status")).toContainText("press Run");
  await expect(fresh.getByLabel("Filename", { exact: true })).toHaveValue("snippet.gcl");
  expect(
    requests.some((request) => request.includes(".wasm") || request.includes("evaluation-worker")),
  ).toBe(false);
  expect(
    requests.some((request) => request.includes("#v=") || request.includes("shared_value")),
  ).toBe(false);
  await fresh.getByRole("button", { name: "Run", exact: true }).click();
  await expect(fresh.locator("#output")).toContainText("shared_value");
  await expect(fresh.locator("#status")).toHaveText("Up to date");
  await context.close();
  await source(page, text + "// edited again\n");
  await expect(page.locator("#share-status")).toContainText("not saved");
  expect(page.url()).toBe(url);
});

test("diagnostics, assertions, plugins and source text remain safe", async ({ page }) => {
  await page.goto(`/playground/${fragment("// 🙂\nnode bad: Length = 1.0 s;")}`);
  await expect(page.locator("#status")).toContainText("press Run");
  await page.getByRole("button", { name: "Run", exact: true }).click();
  await expect(page.locator("#status")).toHaveText("Compile error");
  await page.locator("#output article button").first().click();
  await expect(page.getByRole("textbox", { name: "Graphcal source editor" })).toBeFocused();
  await source(
    page,
    "param x: Dimensionless = 0.0; node y: Dimensionless = 1.0 / @x; node z: Dimensionless = @y; assert check = false;",
  );
  await page.getByRole("button", { name: "Run", exact: true }).click();
  await expect(page.locator("#status")).toHaveText("Completed with errors");
  await expect(page.locator("#output")).toContainText("Dependency failed");
  await expect(page.locator("#output")).toContainText("FAIL");
  await source(
    page,
    'import plugin "graphcal:demo" as demo { fn f(x: Dimensionless) -> Dimensionless; }',
  );
  await page.getByRole("button", { name: "Run", exact: true }).click();
  await expect(page.locator("#status")).toHaveText("Source rejected");
  await expect(page.locator("#output")).toContainText("plugin");
});

test("parameter edits update unit-aware results without stale output", async ({ page }) => {
  await ready(page);
  await source(page, "param distance: Length = 3.0 m; node doubled: Length = @distance * 2.0;");
  await page.getByRole("button", { name: "Run", exact: true }).click();
  await expect(page.locator("#status")).toHaveText("Up to date");
  await expect(page.locator("#output")).toContainText("6 m");
  await source(page, "param distance: Length = 4.0 m; node doubled: Length = @distance * 2.0;");
  await expect(page.locator("#output")).not.toContainText("6 m");
  await page.getByRole("button", { name: "Run", exact: true }).click();
  await expect(page.locator("#status")).toHaveText("Up to date");
  await expect(page.locator("#output")).toContainText("8 m");
});

test("dirty example replacement and Reset require confirmation", async ({ page }) => {
  await ready(page);
  await source(page, "node edited: Int = 3;");
  await page.getByLabel("Examples", { exact: true }).selectOption("hello");
  page.once("dialog", (dialog) => dialog.dismiss());
  await page.getByRole("button", { name: "Load example", exact: true }).click();
  await expect(page.getByRole("textbox", { name: "Graphcal source editor" })).toContainText(
    "edited",
  );
  page.once("dialog", (dialog) => dialog.accept());
  await page.getByRole("button", { name: "Reset", exact: true }).click();
  await expect(page.getByRole("textbox", { name: "Graphcal source editor" })).toContainText(
    "dry_mass",
  );
});

test("malformed links and example loading failures do not replace work", async ({ page }) => {
  await page.goto("/playground/?example=rocket#v=3&code=broken");
  await expect(page.locator("#status")).toContainText("Unsupported");
  await expect(page.getByRole("textbox", { name: "Graphcal source editor" })).toHaveText("");
  await page.getByLabel("Examples", { exact: true }).selectOption("rocket");
  await page.getByRole("button", { name: "Load example", exact: true }).click();
  await expect(page.locator("#status")).toHaveText("Up to date");
  await page.route("**/examples/hello.gcl", (route) =>
    route.fulfill({ status: 503, body: "Unavailable" }),
  );
  await page.getByLabel("Examples", { exact: true }).selectOption("hello");
  await page.getByRole("button", { name: "Load example", exact: true }).click();
  await expect(page.locator("#status")).toContainText("503");
  await expect(page.getByRole("textbox", { name: "Graphcal source editor" })).toContainText(
    "dry_mass",
  );
});

test("worker load failure and Stop recover on the next Run", async ({ page, context }) => {
  await context.route("**/pkg/graphcal_wasm_bg.wasm", (route) => route.abort());
  await page.goto("/playground/");
  await expect(page.locator("#status")).toContainText(/failed|fetch|load|Network/i);
  await context.unroute("**/pkg/graphcal_wasm_bg.wasm");
  await page.getByRole("button", { name: "Run", exact: true }).click();
  await expect(page.locator("#status")).toHaveText("Up to date");
  // Stop a fresh worker while its engine download is held in flight.
  await source(page, "node x: Int = 1;");
  await context.route("**/pkg/graphcal_wasm_bg.wasm", async (route) => {
    await new Promise((resolve) => setTimeout(resolve, 500));
    await route.continue().catch(() => {});
  });
  // A running worker is always cancellable; create one by navigating to shared source.
  page.once("dialog", (dialog) => dialog.accept());
  await page.goto(`/playground/${fragment("node stopped: Int = 2;")}`);
  await expect(page.locator("#status")).toContainText("press Run");
  await page.getByRole("button", { name: "Run", exact: true }).click();
  await page.getByRole("button", { name: "Stop", exact: true }).click();
  await expect(page.locator("#status")).toHaveText("Stopped");
  await context.unroute("**/pkg/graphcal_wasm_bg.wasm");
  await page.getByRole("button", { name: "Run", exact: true }).click();
  await expect(page.locator("#status")).toHaveText("Up to date");
});

test("mobile panes and trailing-slash redirect preserve shared source", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const hash = fragment("node mobile_value: Int = 42;");
  await page.goto(`/playground${hash}`);
  await expect(page).toHaveURL(
    new RegExp(`/playground/${hash.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`),
  );
  await expect(page.locator("#status")).toContainText("press Run");
  // View controls consume space too; the editor must fill the remaining pane,
  // independent of browser-specific toolbar wrapping and font metrics.
  const editorBox = (await page.locator("#editor").boundingBox())!;
  const workspaceBox = (await page.locator("#workspace").boundingBox())!;
  const footerBox = (await page.locator("footer").boundingBox())!;
  expect(editorBox.height).toBeGreaterThan(0);
  expect(editorBox.y).toBeCloseTo(workspaceBox.y, 0);
  expect(editorBox.y + editorBox.height).toBeCloseTo(footerBox.y, 0);
  await page.getByRole("button", { name: "Run", exact: true }).click();
  await expect(page.locator("#status")).toHaveText("Up to date");
  await page.getByRole("button", { name: "Results", exact: true }).click();
  await expect(page.locator("#output")).toBeVisible();
  await expect(page.locator("#output")).toContainText("mobile_value");
  await page.getByRole("button", { name: "Editor", exact: true }).click();
  await expect(page.locator("#editor")).toBeVisible();
  await page.screenshot({ path: "test-results/playground-mobile.png" });
});
