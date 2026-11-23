// A tooltip of the page's own, for text a reader must be able to see at once:
// the native `title` waits a second in a browser and does not show at all in
// a VS Code webview. An element names its text in `data-tip` (see `setTip`).

/** How long a hint waits before it shows, so that it stays out of the way of a passing pointer. */
const HINT_WAIT_MS = 500;

/**
 * Give `element` a tip. A `hint` (what a click does, what a region is) waits
 * for the pointer to rest; anything else, like why a value is missing, shows
 * at once.
 */
export function setTip(element: HTMLElement, text: string, kind: "now" | "hint" = "now"): void {
  element.dataset.tip = text;
  if (kind === "hint") element.dataset.tipWait = "";
  else delete element.dataset.tipWait;
}

/** Where the tip's box goes: under the anchor, or above it when there is no room, kept inside the viewport. */
export function tipPlace(
  anchor: { left: number; top: number; bottom: number },
  tip: { width: number; height: number },
  viewport: { width: number; height: number },
  gap = 6,
): { left: number; top: number } {
  const below = anchor.bottom + gap;
  const top = below + tip.height <= viewport.height ? below : anchor.top - gap - tip.height;
  const left = Math.min(anchor.left, viewport.width - tip.width - gap);
  return { left: Math.max(gap, left), top: Math.max(gap, top) };
}

/** Show the `data-tip` of the element under the pointer, as soon as the pointer is over it. */
export function mountTips(root: Document): void {
  const tip = root.createElement("div");
  tip.className = "tip";
  tip.hidden = true;
  root.body.append(tip);
  let shown: HTMLElement | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const hide = (): void => {
    clearTimeout(timer);
    shown = null;
    tip.hidden = true;
  };
  const show = (anchor: HTMLElement): void => {
    // The sheet may have redrawn while a hint waited.
    if (!anchor.isConnected) return;
    tip.textContent = anchor.dataset.tip ?? "";
    tip.hidden = false;
    const place = tipPlace(anchor.getBoundingClientRect(), tip.getBoundingClientRect(), {
      width: root.documentElement.clientWidth,
      height: root.documentElement.clientHeight,
    });
    tip.style.left = `${place.left}px`;
    tip.style.top = `${place.top}px`;
  };
  root.addEventListener("pointerover", (event) => {
    const target = event.target instanceof Element ? event.target : null;
    const anchor = target?.closest<HTMLElement>("[data-tip]") ?? null;
    if (anchor === shown) return;
    hide();
    if (anchor === null) return;
    shown = anchor;
    if (anchor.dataset.tipWait === undefined) show(anchor);
    else timer = setTimeout(() => show(anchor), HINT_WAIT_MS);
  });
  // The pointer left the page: no `pointerover` follows to hide the tip.
  root.documentElement.addEventListener("pointerleave", hide);
  // The sheet redraws and pans under a still pointer: a tip must not outlive its anchor.
  root.addEventListener("pointerdown", hide);
  root.addEventListener("wheel", hide, { passive: true });
  root.addEventListener("keydown", hide);
}
