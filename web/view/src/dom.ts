// Small DOM helpers the page's modules share.

export const SVG_NS = "http://www.w3.org/2000/svg";

/** The shell's element for `selector`; the page cannot run without it. */
export function need<T extends Element>(selector: string): T {
  const found = document.querySelector<T>(selector);
  if (!found) throw new Error(`view shell is missing ${selector}`);
  return found;
}

/** A new `tag` element with `className` and, when given, `text`. */
export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
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
