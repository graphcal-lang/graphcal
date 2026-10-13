// The explorer: the files of the shown project as a tree, and (when the host
// reads a repository) the ref being read, with a menu of the branches, tags
// and pull requests to switch to. It opens from the breadcrumb at the top
// left and closes from it again.
import type { Refs } from "./github.ts";

export interface TreeRow {
  /** Project path (`src/power_project/eps.gcl`); a directory's path has no trailing slash. */
  path: string;
  name: string;
  depth: number;
  kind: "dir" | "file";
}

/** The rows of a tree listing `files`: directories first at each level, names in order. */
export function fileTree(files: readonly string[]): TreeRow[] {
  interface Dir {
    dirs: Map<string, Dir>;
    files: string[];
  }
  const root: Dir = { dirs: new Map(), files: [] };
  for (const file of files) {
    const parts = file.split("/");
    let dir = root;
    for (const part of parts.slice(0, -1)) {
      let next = dir.dirs.get(part);
      if (!next) {
        next = { dirs: new Map(), files: [] };
        dir.dirs.set(part, next);
      }
      dir = next;
    }
    dir.files.push(parts.at(-1) ?? file);
  }
  const rows: TreeRow[] = [];
  const walk = (dir: Dir, prefix: string, depth: number): void => {
    for (const name of [...dir.dirs.keys()].sort()) {
      const path = prefix === "" ? name : `${prefix}/${name}`;
      rows.push({ path, name, depth, kind: "dir" });
      walk(dir.dirs.get(name) as Dir, path, depth + 1);
    }
    for (const name of [...dir.files].sort()) {
      rows.push({ path: prefix === "" ? name : `${prefix}/${name}`, name, depth, kind: "file" });
    }
  };
  walk(root, "", 0);
  return rows;
}

/** The ref part of the explorer, when the host reads a repository. */
export interface RefSource {
  /** The ref being read, once known. */
  current: string | null;
  list(): Promise<Refs>;
  /** Read the project again at `ref`. */
  choose(ref: string): void;
  /** Whether choosing a ref would discard what-if edits. */
  edited(): boolean;
}

export interface ExplorerOptions {
  /** `owner / repo`, or the package name. */
  title: string;
  refs: RefSource | null;
  onOpen(file: string): void;
}

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className: string,
  text = "",
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  node.className = className;
  if (text !== "") node.textContent = text;
  return node;
}

export class Explorer {
  private readonly root: HTMLElement;
  private readonly titleEl: HTMLElement;
  private readonly refEl: HTMLButtonElement | null;
  private readonly menuEl: HTMLElement | null;
  private readonly treeEl: HTMLElement;
  private readonly options: ExplorerOptions;
  private refs: Promise<Refs> | null = null;
  private files: readonly string[] = [];
  private selected: string | null = null;

  constructor(root: HTMLElement, options: ExplorerOptions) {
    this.root = root;
    this.options = options;
    this.titleEl = el("div", "explorer-title", options.title);
    this.treeEl = el("div", "explorer-tree");
    root.replaceChildren(this.titleEl);
    if (options.refs) {
      const row = el("div", "explorer-ref-row");
      this.refEl = el("button", "explorer-ref");
      this.refEl.type = "button";
      this.refEl.title = "switch branch, tag or pull request";
      this.refEl.addEventListener("click", () => this.toggleMenu());
      row.append(this.refEl);
      this.menuEl = el("div", "refs-menu");
      this.menuEl.hidden = true;
      root.append(row, this.menuEl);
      this.renderRef();
      document.addEventListener("pointerdown", (event) => {
        const target = event.target as Node;
        if (this.menuEl && !this.menuEl.hidden && !this.menuEl.contains(target)) {
          if (!this.refEl?.contains(target)) this.closeMenu();
        }
      });
      document.addEventListener("keydown", (event) => {
        if (event.key === "Escape") this.closeMenu();
      });
    } else {
      this.refEl = null;
      this.menuEl = null;
    }
    root.append(this.treeEl);
    this.treeEl.addEventListener("click", (event) => {
      const row = (event.target as HTMLElement).closest<HTMLElement>(".tree-row.file");
      if (row?.dataset.path) this.options.onOpen(row.dataset.path);
    });
  }

  /** The ref being read changed (it is resolved after the route loads). */
  renderRef(): void {
    if (!this.refEl || !this.options.refs) return;
    const current = this.options.refs.current;
    this.refEl.replaceChildren(
      el("span", "ref-mark", "⎇"),
      el("span", "ref-name", current ?? "…"),
      el("span", "ref-chevron", "⌄"),
    );
  }

  /** Show `files` of the project, with `selected` marked. */
  render(files: readonly string[], selected: string | null): void {
    this.files = files;
    this.selected = selected;
    this.treeEl.replaceChildren();
    for (const row of fileTree(files)) {
      const rowEl = el("div", `tree-row ${row.kind}`);
      rowEl.dataset.path = row.path;
      rowEl.style.paddingLeft = `${14 + row.depth * 14}px`;
      if (row.kind === "dir") rowEl.append(el("span", "tree-mark", "▾"));
      rowEl.append(el("span", "tree-name", row.name));
      if (row.path === selected) rowEl.classList.add("selected");
      if (row.kind === "file" && !row.name.endsWith(".gcl")) rowEl.classList.add("other");
      this.treeEl.append(rowEl);
    }
  }

  /** Re-render with what was shown last (after the title or the ref changed). */
  refresh(): void {
    this.render(this.files, this.selected);
  }

  private toggleMenu(): void {
    if (!this.menuEl) return;
    if (!this.menuEl.hidden) {
      this.closeMenu();
      return;
    }
    this.menuEl.hidden = false;
    this.refEl?.classList.add("open");
    this.menuEl.replaceChildren(el("div", "refs-note", "reading refs…"));
    this.refs ??= this.options.refs?.list() ?? Promise.reject(new Error("no refs"));
    this.refs.then(
      (refs) => this.renderMenu(refs),
      (error: unknown) => {
        this.refs = null;
        const why = error instanceof Error ? error.message : String(error);
        this.menuEl?.replaceChildren(el("div", "refs-note error", why));
      },
    );
  }

  closeMenu(): void {
    if (!this.menuEl) return;
    this.menuEl.hidden = true;
    this.refEl?.classList.remove("open");
  }

  private renderMenu(refs: Refs): void {
    const menu = this.menuEl;
    const source = this.options.refs;
    if (!menu || !source || menu.hidden) return;
    const filter = el("input", "refs-filter");
    filter.type = "search";
    filter.placeholder = "filter branches, tags, PRs…";
    filter.setAttribute("aria-label", "filter refs");
    const list = el("div", "refs-list");
    const foot = el("div", "refs-foot");
    const renderList = (): void => {
      const needle = filter.value.trim().toLowerCase();
      const matches = (text: string): boolean =>
        needle === "" || text.toLowerCase().includes(needle);
      list.replaceChildren();
      const section = (
        label: string,
        items: { label: string; ref: string; hint: string }[],
      ): void => {
        const shown = items.filter((item) => matches(item.label));
        if (shown.length === 0) return;
        // The ref being read first, so it is in sight however long the list.
        shown.sort((a, b) => Number(b.ref === source.current) - Number(a.ref === source.current));
        list.append(el("div", "refs-label", label));
        for (const item of shown) {
          const entry = el("button", "refs-item");
          entry.type = "button";
          const current = item.ref === source.current;
          entry.classList.toggle("current", current);
          entry.append(el("span", "refs-item-name", item.label));
          entry.append(el("span", "refs-item-hint", current ? "✓" : item.hint));
          entry.addEventListener("click", () => {
            this.closeMenu();
            if (!current) source.choose(item.ref);
          });
          list.append(entry);
        }
      };
      section(
        "branches",
        refs.branches.map((b) => ({ label: b, ref: b, hint: "" })),
      );
      section(
        "tags",
        refs.tags.map((t) => ({ label: t, ref: t, hint: "" })),
      );
      section(
        "pull requests",
        refs.pulls.map((p) => ({ label: `#${p.number}  ${p.title}`, ref: p.ref, hint: "" })),
      );
      if (list.childElementCount === 0) list.append(el("div", "refs-note", "nothing matches"));
    };
    filter.addEventListener("input", renderList);
    filter.addEventListener("keydown", (event) => {
      if (event.key === "Escape") this.closeMenu();
    });
    foot.textContent = source.edited()
      ? "switching discards the what-if edits"
      : "the address follows: #owner/repo@ref/path";
    renderList();
    menu.replaceChildren(filter, list, foot);
    filter.focus();
  }

  /** Whether the explorer is shown. */
  get open(): boolean {
    return !this.root.hidden;
  }

  set open(value: boolean) {
    this.root.hidden = !value;
    if (!value) this.closeMenu();
  }

  setTitle(title: string): void {
    this.titleEl.textContent = title;
  }
}
