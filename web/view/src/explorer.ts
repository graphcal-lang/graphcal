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

/**
 * The rows of a tree listing `files`: directories first at each level, names
 * in order. A directory holding one directory and nothing else is shown with
 * it on one row (`src/power_project`), as VS Code compacts folders.
 */
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
    for (const first of [...dir.dirs.keys()].sort()) {
      let name = first;
      let path = prefix === "" ? first : `${prefix}/${first}`;
      let child = dir.dirs.get(first) as Dir;
      while (child.files.length === 0 && child.dirs.size === 1) {
        const [only] = child.dirs.keys() as Iterable<string>;
        name = `${name}/${only}`;
        path = `${path}/${only}`;
        child = child.dirs.get(only as string) as Dir;
      }
      rows.push({ path, name, depth, kind: "dir" });
      walk(child, path, depth + 1);
    }
    for (const name of [...dir.files].sort()) {
      rows.push({ path: prefix === "" ? name : `${prefix}/${name}`, name, depth, kind: "file" });
    }
  };
  walk(root, "", 0);
  return rows;
}

/** The rows shown when the directories in `collapsed` are folded: nothing under them. */
export function visibleRows(rows: readonly TreeRow[], collapsed: ReadonlySet<string>): TreeRow[] {
  const out: TreeRow[] = [];
  let hiddenBelow: number | null = null;
  for (const row of rows) {
    if (hiddenBelow !== null) {
      if (row.depth > hiddenBelow) continue;
      hiddenBelow = null;
    }
    out.push(row);
    if (row.kind === "dir" && collapsed.has(row.path)) hiddenBelow = row.depth;
  }
  return out;
}

/** The directories above `path` (`a/b/c.gcl` → `a`, `a/b`). */
function ancestors(path: string): string[] {
  const parts = path.split("/");
  return parts.slice(0, -1).map((_, i) => parts.slice(0, i + 1).join("/"));
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
  /** The repository or package name. */
  title: string;
  /** Shown as the title's tooltip (`owner/repo`). */
  subtitle: string;
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
  /** Directories folded by hand, or never opened; the selected file's are always open. */
  private collapsed = new Set<string>();
  /** Directories the user has opened or closed, which `render` then leaves alone. */
  private readonly touched = new Set<string>();

  constructor(root: HTMLElement, options: ExplorerOptions) {
    this.root = root;
    this.options = options;
    this.titleEl = el("div", "explorer-title", options.title);
    this.titleEl.title = options.subtitle;
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
      const row = (event.target as HTMLElement).closest<HTMLElement>(".tree-row");
      const path = row?.dataset.path;
      if (!row || path === undefined) return;
      if (row.classList.contains("dir")) {
        this.touched.add(path);
        if (this.collapsed.has(path)) this.collapsed.delete(path);
        else this.collapsed.add(path);
        this.refresh();
      } else if (row.classList.contains("openable")) {
        this.options.onOpen(path);
      }
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

  /**
   * Show `files` as a tree with `selected` marked. Directories start folded
   * except those above the selected file; what the user folds or unfolds stays so.
   */
  render(files: readonly string[], selected: string | null): void {
    if (files !== this.files) {
      const rows = fileTree(files);
      const known = new Set(rows.filter((r) => r.kind === "dir").map((r) => r.path));
      for (const dir of known) if (!this.touched.has(dir)) this.collapsed.add(dir);
      for (const dir of this.collapsed) if (!known.has(dir)) this.collapsed.delete(dir);
    }
    this.files = files;
    this.selected = selected;
    if (selected !== null) for (const dir of ancestors(selected)) this.collapsed.delete(dir);
    this.treeEl.replaceChildren();
    for (const row of visibleRows(fileTree(files), this.collapsed)) {
      const rowEl = el("div", `tree-row ${row.kind}`);
      rowEl.dataset.path = row.path;
      rowEl.style.paddingLeft = `${10 + row.depth * 14}px`;
      if (row.kind === "dir") {
        const folded = this.collapsed.has(row.path);
        rowEl.classList.toggle("folded", folded);
        rowEl.append(el("span", "tree-mark", folded ? "▸" : "▾"));
        rowEl.title = folded ? "open" : "close";
      } else {
        rowEl.append(el("span", "tree-mark", ""));
        if (row.name.endsWith(".gcl")) rowEl.classList.add("openable");
        else rowEl.classList.add("other");
      }
      rowEl.append(el("span", "tree-name", row.name));
      if (row.path === selected) rowEl.classList.add("selected");
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
