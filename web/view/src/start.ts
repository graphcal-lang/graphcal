// The page without an address: nothing to show yet, so it asks for a file.
// A GitHub file URL or an `owner/repo/path.gcl` pasted here becomes the
// address, and the page loads it.
import type { Host } from "./host.ts";
import type { ViewModel } from "./model.ts";
import { formatRoute, routeFromText } from "./route.ts";

/** A host with nothing in it, for the page that asks for a file. */
export class EmptyHost implements Host {
  readonly modules: readonly string[] = [];
  readonly edited = false;
  model(): ViewModel | null {
    return null;
  }
  source(): string | null {
    return null;
  }
  edit(): Promise<{ ok: false; message: string }> {
    return Promise.resolve({ ok: false, message: "nothing is open" });
  }
  request(): void {}
  reset(): void {}
  onModels(): void {}
  onOpen(): void {}
  onNote(): void {}
}

/** Fill the start panel: a field for the address and what it accepts. */
export function mountStart(root: HTMLElement, badAddress: string | null): void {
  const form = document.createElement("form");
  form.className = "start-form";
  const title = document.createElement("h1");
  title.className = "start-title";
  title.textContent = "Open a Graphcal file from GitHub";
  const text = document.createElement("p");
  text.className = "start-text";
  text.textContent =
    "Paste the file's GitHub URL, or owner/repo/path/to/file.gcl (add @branch for a branch, @base...head to see what changed between two refs). The project around the file is read and evaluated here, in your browser.";
  const input = document.createElement("input");
  input.className = "start-input";
  input.type = "text";
  input.placeholder = "https://github.com/owner/repo/blob/main/src/file.gcl";
  input.spellcheck = false;
  input.setAttribute("aria-label", "file address");
  const row = document.createElement("div");
  row.className = "start-row";
  const button = document.createElement("button");
  button.type = "submit";
  button.className = "start-open";
  button.textContent = "Open";
  const note = document.createElement("span");
  note.className = "start-note";
  if (badAddress !== null) {
    note.textContent = `not a file address: ${badAddress}`;
    input.value = badAddress;
  }
  row.append(button, note);
  form.append(title, text, input, row);
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const route = routeFromText(input.value);
    if (route === null) {
      note.textContent = "expected a GitHub file URL or owner/repo/path.gcl";
      return;
    }
    location.hash = formatRoute(route);
    location.reload();
  });
  root.replaceChildren(form);
  root.hidden = false;
  input.focus();
}
