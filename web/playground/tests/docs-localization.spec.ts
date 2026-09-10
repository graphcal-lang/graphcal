import { test, expect, type Page } from "@playwright/test";
import { readFile } from "node:fs/promises";

async function search(page: Page, query: string, locale: "en" | "ja") {
  // Disco's input handles keyboard events; exercise real typing, not only an
  // input-value assignment. Do not depend on its minified CSS class names.
  await page.locator(".md-search__button").click();
  const input = page.getByRole("combobox");
  await input.fill("");
  await input.pressSequentially(query);
  const results = page.locator('a[href*="?h="]');
  await expect(results.first()).toBeVisible();
  const paths = await results.evaluateAll((links) =>
    links.map((link) => new URL((link as HTMLAnchorElement).href).pathname),
  );
  expect(paths.length).toBeGreaterThan(0);
  for (const path of paths) {
    expect(path.startsWith("/docs/")).toBe(true);
    expect(path.startsWith("/docs/ja/")).toBe(locale === "ja");
  }
  return results;
}

test("language home selector is keyboard accessible and changes the document language", async ({
  page,
}) => {
  await page.goto("/docs/");
  const header = page.locator("header");
  const selector = header.getByRole("button", { name: "Select language", exact: true });
  await selector.focus();
  await selector.press("Enter");
  const japanese = header.getByRole("link", { name: "日本語", exact: true });
  await expect(japanese).toBeVisible();
  await japanese.focus();
  await japanese.press("Enter");
  await expect(page).toHaveURL(/\/docs\/ja\/$/);
  await expect(page.locator("html")).toHaveAttribute("lang", "ja");
  await expect(page.locator("article h1")).toContainText("Graphcal へようこそ");
  await header.getByRole("button", { name: "言語切り替え", exact: true }).click();
  await header.getByRole("link", { name: "English", exact: true }).click();
  await expect(page).toHaveURL(/\/docs\/$/);
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
});

test("deep counterpart links, back/forward, and search do not reuse the other locale", async ({
  page,
}) => {
  const liveRequests: string[] = [];
  const errors: string[] = [];
  page.on("request", (request) => {
    if (new URL(request.url()).hostname === "graphcal.org") liveRequests.push(request.url());
  });
  page.on("pageerror", (error) => errors.push(error.message));
  const route = "language/dimensions-and-units/";
  await page.goto(`/docs/${route}`);
  await search(page, "units", "en");
  await page.keyboard.press("Escape");
  await page.locator(".doc-translation").getByRole("link", { name: "日本語" }).click();
  await expect(page).toHaveURL(new RegExp(`/docs/ja/${route}$`));
  await expect(page.locator("html")).toHaveAttribute("lang", "ja");
  await search(page, "単位", "ja");
  await page.keyboard.press("Escape");
  await page.goBack();
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await expect(page).toHaveURL(new RegExp(`/docs/${route}$`));
  await page.goForward();
  await expect(page.locator("html")).toHaveAttribute("lang", "ja");
  await page.locator(".doc-translation").getByRole("link", { name: "English" }).click();
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await search(page, "units", "en");
  expect(liveRequests).toEqual([]);
  expect(errors).toEqual([]);
});

for (const query of ["単位", "次元", "型", "Dimensionless"]) {
  test(`Japanese search finds ${query} and opens a Japanese result`, async ({ page }) => {
    await page.goto("/docs/ja/");
    const results = await search(page, query, "ja");
    await expect(results.first()).toContainText(query);
    await results.first().click();
    await expect(page).toHaveURL(/\/docs\/ja\/.+[?]h=/);
    await expect(page.locator("html")).toHaveAttribute("lang", "ja");
    await expect(page.locator("article h1")).toBeVisible();
    const fragment = new URL(page.url()).hash.slice(1);
    if (fragment) {
      expect(
        await page.evaluate(
          (id) => document.getElementById(decodeURIComponent(id)) !== null,
          fragment,
        ),
      ).toBe(true);
    }
  });
}

test("Japanese tutorial uses the canonical downloadable source and working playground", async ({
  page,
}) => {
  await page.goto("/docs/ja/tutorial/step1-hello-graphcal/");
  const sourcePath = "/docs/assets/playground/examples/step-1/main.gcl";
  await expect(page.locator(`article a[href$="${sourcePath}"]`)).toBeVisible();
  const response = await page.request.get(sourcePath);
  expect(response.ok()).toBe(true);
  expect(await response.text()).toBe(
    await readFile(
      new URL("../../../docs/en/assets/playground/examples/step-1/main.gcl", import.meta.url),
      "utf8",
    ),
  );
  const example = page.locator('article a[href="https://graphcal.org/playground/?example=hello"]');
  await expect(example).toBeVisible();
  // Resolve the production URL against this artifact, not the live site.
  await page.goto("/playground/?example=hello");
  await expect(page.locator("#status")).toHaveText("Up to date");
  await expect(page.locator("#output")).toContainText("mass_with_margin");
});

test("Japanese mobile layout, shared image, code controls, and theme remain usable", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/docs/ja/");
  const image = page.locator('article img[src$="/docs/assets/rocket-screenshot.png"]');
  await expect(image).toBeVisible();
  await expect
    .poll(() => image.evaluate((image: HTMLImageElement) => image.naturalWidth))
    .toBeGreaterThan(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(391);
  const copy = page.locator('button[data-md-type="copy"]').first();
  await copy.scrollIntoViewIfNeeded();
  await expect(copy).toHaveAttribute("title", "クリップボードへコピー");
  await expect(copy).toBeVisible();
  await page.locator('label[for="__palette_1"]').click();
  await expect(page.locator("body")).toHaveAttribute("data-md-color-scheme", "slate");
  await page.locator(".doc-translation").getByRole("link", { name: "English" }).click();
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
});
