import { required } from "./dom";

export function setupLayout() {
  const workspace = required("#workspace");
  const separator = required("#separator");
  let percent = 60;
  function resize(value: number) {
    percent = Math.max(25, Math.min(75, value));
    workspace.style.setProperty("--editor-width", `${percent}%`);
    separator.setAttribute("aria-valuenow", String(Math.round(percent)));
  }
  separator.addEventListener("keydown", (event) => {
    if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      event.preventDefault();
      resize(percent + (event.key === "ArrowLeft" ? -5 : 5));
    }
    if (event.key === "Home" || event.key === "End") {
      event.preventDefault();
      resize(event.key === "Home" ? 25 : 75);
    }
  });
  separator.addEventListener("pointerdown", (event) => {
    separator.setPointerCapture(event.pointerId);
  });
  separator.addEventListener("pointermove", (event) => {
    if (separator.hasPointerCapture(event.pointerId)) {
      const bounds = workspace.getBoundingClientRect();
      resize((100 * (event.clientX - bounds.left)) / bounds.width);
    }
  });
  const editorTab = required<HTMLButtonElement>("#editor-tab");
  const resultsTab = required<HTMLButtonElement>("#results-tab");
  function showEditor(editor: boolean) {
    workspace.dataset.pane = editor ? "editor" : "results";
    editorTab.setAttribute("aria-pressed", String(editor));
    resultsTab.setAttribute("aria-pressed", String(!editor));
  }
  editorTab.addEventListener("click", () => showEditor(true));
  resultsTab.addEventListener("click", () => showEditor(false));
  required("#theme").addEventListener("click", () => {
    const dark = getComputedStyle(document.documentElement).colorScheme === "dark";
    document.documentElement.dataset.theme = dark ? "light" : "dark";
  });
  return { showEditor: () => showEditor(true) };
}
