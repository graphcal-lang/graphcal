import type { Page } from "@playwright/test";

export function sourceEditor(page: Page) {
  return page.getByRole("textbox", { name: "Graphcal source editor" });
}

export async function replaceSource(page: Page, source: string) {
  const editor = sourceEditor(page);
  // Avoid Playwright's cross-line contenteditable fill selection, which Firefox reports as a
  // composition and CodeMirror 6.43.12 removes, exposing a Playwright/Firefox fill incompatibility.
  await editor.press("ControlOrMeta+A");
  await editor.press("Backspace");
  await editor.fill(source);
}
