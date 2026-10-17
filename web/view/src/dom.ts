// Small DOM helpers the page's modules share.

export const SVG_NS = "http://www.w3.org/2000/svg";

/** The shell's element for `selector`; the page cannot run without it. */
export function need<T extends Element>(selector: string): T {
  const found = document.querySelector<T>(selector);
  if (!found) throw new Error(`view shell is missing ${selector}`);
  return found;
}

export function span(className: string, textContent: string): HTMLSpanElement {
  return Object.assign(document.createElement("span"), { className, textContent });
}

export function button(className: string, textContent: string, title: string): HTMLButtonElement {
  return Object.assign(document.createElement("button"), {
    type: "button",
    className,
    textContent,
    title,
  });
}
