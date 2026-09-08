import { test, expect } from "@playwright/test";
import { gzipSync, gunzipSync } from "node:zlib";
import { randomBytes } from "node:crypto";

const shared = (source: string) =>
  `#v=1&code=${gzipSync(JSON.stringify({ filename: "shared.gcl", source })).toString("base64url")}`;

test("Back and fragment navigation respect dirty-document confirmation", async ({ page }) => {
  await page.goto("/playground/");
  await expect(page.locator("#status")).toHaveText("Up to date");
  await page.getByLabel("Examples", { exact: true }).selectOption("hello");
  await page.getByRole("button", { name: "Load example", exact: true }).click();
  await expect(page).toHaveURL(/example=hello/);
  await expect(page.locator("#status")).toHaveText("Up to date");
  await page.goBack();
  await expect(page.getByLabel("Filename", { exact: true })).toHaveValue("rocket.gcl");
  await expect(page.locator("#status")).toHaveText("Up to date");
  await page.getByLabel("Auto-run", { exact: true }).uncheck();
  await page
    .getByRole("textbox", { name: "Graphcal source editor" })
    .fill("node keep_me: Int = 1;");
  const previous = page.url();
  const hash = shared("node incoming: Int = 2;");
  page.once("dialog", (dialog) => dialog.dismiss());
  await page.evaluate((hash) => {
    window.location.hash = hash;
  }, hash);
  await expect(page).toHaveURL(previous);
  await expect(page.getByRole("textbox", { name: "Graphcal source editor" })).toContainText(
    "keep_me",
  );
  page.once("dialog", (dialog) => dialog.accept());
  await page.evaluate((hash) => {
    window.location.hash = hash;
  }, hash);
  await expect(page.locator("#status")).toContainText("press Run");
  await expect(page.getByLabel("Filename", { exact: true })).toHaveValue("shared.gcl");
  await expect(page.getByRole("textbox", { name: "Graphcal source editor" })).toContainText(
    "incoming",
  );
});

test("a delayed example cannot overwrite newer edits or a shared snapshot", async ({ page }) => {
  await page.goto("/playground/");
  await expect(page.locator("#status")).toHaveText("Up to date");
  let release!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  let requested!: () => void;
  const seen = new Promise<void>((resolve) => {
    requested = resolve;
  });
  await page.route("**/examples/hello.gcl", async (route) => {
    requested();
    await held;
    await route.continue().catch(() => {});
  });
  await page.getByLabel("Examples", { exact: true }).selectOption("hello");
  await page.getByRole("button", { name: "Load example", exact: true }).click();
  await seen;
  await page.getByRole("textbox", { name: "Graphcal source editor" }).fill("node newer: Int = 9;");
  await page.getByRole("button", { name: "Share", exact: true }).click();
  await expect(page.getByRole("textbox", { name: "Shareable URL" })).toBeVisible();
  release();
  await page.getByRole("button", { name: "Run", exact: true }).click();
  await expect(page.locator("#status")).toHaveText("Up to date");
  await expect(page.locator("#output")).toContainText("newer");
  const encoded = new URL(page.url()).hash.slice(1);
  const bytes = Buffer.from(new URLSearchParams(encoded).get("code")!, "base64url");
  expect(JSON.parse(gunzipSync(bytes).toString("utf8")).source).toBe("node newer: Int = 9;");
});

test("clipboard denial offers a selected URL and oversized sharing keeps source", async ({
  page,
}) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: () => Promise.reject(new Error("Clipboard denied")) },
    });
  });
  await page.goto(`/playground/${shared("// 🙂\r\nnode x: Int = 3;\n")}`);
  await expect(page.locator("#status")).toContainText("press Run");
  await page.getByRole("button", { name: "Share", exact: true }).click();
  const link = page.getByRole("textbox", { name: "Shareable URL" });
  await expect(link).toBeFocused();
  expect(
    await link.evaluate((node: HTMLInputElement) => node.selectionEnd! - node.selectionStart!),
  ).toBe((await link.inputValue()).length);
  const payload = new URLSearchParams(new URL(page.url()).hash.slice(1)).get("code")!;
  expect(JSON.parse(gunzipSync(Buffer.from(payload, "base64url")).toString("utf8")).source).toBe(
    "// 🙂\r\nnode x: Int = 3;\n",
  );
  const originalUrl = page.url();
  const large = `// ${randomBytes(25_000).toString("base64")}\nnode retained: Int = 7;`;
  await page.getByRole("textbox", { name: "Graphcal source editor" }).fill(large);
  await page.getByRole("button", { name: "Share", exact: true }).click();
  await expect(page.locator("#share-status")).toContainText(/limit|16 KiB/);
  expect(page.url()).toBe(originalUrl);
  await page.getByRole("button", { name: "Run", exact: true }).click();
  await expect(page.locator("#status")).toHaveText("Up to date");
  await expect(page.locator("#output")).toContainText("retained");
});
