import { test, expect } from "@playwright/test";

test("documentation root redirects to the English edition @smoke", async ({ page }) => {
  await page.goto("/docs/");
  await expect(page).toHaveURL(/\/docs\/en\/$/);
  await expect(page.locator("article h1")).toBeVisible();
});

test("playground Docs link opens the English edition @smoke", async ({ page }) => {
  await page.goto("/playground/");
  await page.getByRole("link", { name: "Docs", exact: true }).click();
  await expect(page).toHaveURL(/\/docs\/en\/$/);
  await expect(page.locator("article h1")).toBeVisible();
});
