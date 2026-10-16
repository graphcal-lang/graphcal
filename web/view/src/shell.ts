// The web shell around the sheet: which panes are shown (code, sheet or
// both), the split between them, and the explorer's toggle. Remembered per
// browser as conveniences; nothing here is shared or saved.

function recall(key: string): string | null {
  try {
    return localStorage.getItem(`graphcal.view.${key}`);
  } catch {
    return null;
  }
}

function remember(key: string, value: string): void {
  try {
    localStorage.setItem(`graphcal.view.${key}`, value);
  } catch {
    // Storage is blocked: the choice lasts for this page only.
  }
}

/** Which panes are shown: the code, the sheet, or both side by side. */
export type Arrangement = "code" | "both" | "view";
const ARRANGEMENTS: readonly Arrangement[] = ["code", "both", "view"];

export function isArrangement(value: string | null | undefined): value is Arrangement {
  return ARRANGEMENTS.some((a) => a === value);
}

export interface ShellElements {
  work: HTMLElement;
  code: HTMLElement;
  split: HTMLElement;
  sheet: HTMLElement;
  layouts: HTMLElement;
  crumb: HTMLButtonElement;
}

export interface ShellHooks {
  /** The canvas may have changed size. */
  relayout(): void;
  /** The code pane alone is shown: it takes the focus. */
  focusCode(): void;
  /** Show or hide the explorer. */
  setExplorer(open: boolean): void;
}

/** Wire the arrangement buttons, the split and the explorer toggle, as remembered. */
export function mountShell(els: ShellElements, hooks: ShellHooks): void {
  const layoutButtons = [...els.layouts.querySelectorAll<HTMLButtonElement>("[data-layout]")];
  const arrange = (arrangement: Arrangement): void => {
    els.code.hidden = arrangement === "view";
    els.split.hidden = arrangement !== "both";
    els.sheet.hidden = arrangement === "code";
    els.work.classList.toggle("code-only", arrangement === "code");
    for (const b of layoutButtons) b.classList.toggle("active", b.dataset.layout === arrangement);
    remember("arrangement", arrangement);
    if (arrangement === "code") hooks.focusCode();
    hooks.relayout();
  };

  // Drag the split: the code pane keeps the width it is dragged to.
  let splitting = false;
  els.split.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    splitting = true;
    els.split.setPointerCapture(event.pointerId);
    els.work.classList.add("splitting");
    event.preventDefault();
  });
  els.split.addEventListener("pointermove", (event) => {
    if (!splitting) return;
    const left = els.code.getBoundingClientRect().left;
    els.code.style.width = `${Math.round(event.clientX - left)}px`;
    hooks.relayout();
  });
  const endSplit = (): void => {
    if (!splitting) return;
    splitting = false;
    els.work.classList.remove("splitting");
    remember("code-width", String(els.code.getBoundingClientRect().width));
  };
  els.split.addEventListener("pointerup", endSplit);
  els.split.addEventListener("pointercancel", endSplit);

  const storedWidth = Number(recall("code-width"));
  if (Number.isFinite(storedWidth) && storedWidth > 0) els.code.style.width = `${storedWidth}px`;
  els.layouts.hidden = false;
  els.crumb.hidden = false;
  for (const b of layoutButtons) {
    b.addEventListener("click", () => {
      const layout = b.dataset.layout;
      if (isArrangement(layout)) arrange(layout);
    });
  }
  let explorerOpen = false;
  const setExplorer = (open: boolean): void => {
    explorerOpen = open;
    hooks.setExplorer(open);
    els.crumb.classList.toggle("open", open);
    remember("explorer", open ? "open" : "closed");
    hooks.relayout();
  };
  els.crumb.addEventListener("click", () => setExplorer(!explorerOpen));
  setExplorer(recall("explorer") === "open");
  const stored = recall("arrangement");
  arrange(isArrangement(stored) ? stored : "both");
}
