// A context menu: a list of actions at the pointer, closed by a choice, a
// click elsewhere or Escape.
import { button } from "./dom.ts";

export interface MenuItem {
  label: string;
  disabled?: boolean;
  run: () => void;
}

export class Menu {
  private readonly el: HTMLElement;

  constructor() {
    this.el = document.createElement("div");
    this.el.className = "menu";
    this.el.hidden = true;
    document.body.append(this.el);
    document.addEventListener("pointerdown", (event) => {
      if (!this.el.hidden && !this.el.contains(event.target as Node)) this.close();
    });
  }

  close(): void {
    this.el.hidden = true;
  }

  open(x: number, y: number, items: readonly MenuItem[]): void {
    this.el.replaceChildren();
    for (const item of items) {
      const entry = button("", item.label, "");
      entry.disabled = item.disabled ?? false;
      entry.addEventListener("click", () => {
        this.close();
        item.run();
      });
      this.el.append(entry);
    }
    this.el.hidden = false;
    this.el.style.left = `${Math.min(x, window.innerWidth - this.el.offsetWidth - 8)}px`;
    this.el.style.top = `${Math.min(y, window.innerHeight - this.el.offsetHeight - 8)}px`;
  }
}
