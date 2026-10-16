// The web viewer's host: the project around a `.gcl` file is read from GitHub
// with the viewer's own token, kept in memory, and projected by the in-browser
// engine. Nothing is written back; edits are a what-if over the sources read,
// and `reset` returns to them. The address bar follows the shown file.
import { changedFiles, patchOf, type Patch } from "./diff.ts";
import { changedPaths, compareOf, type BaseRead } from "./compareBase.ts";
import {
  loadEngine,
  packageLayout,
  projectLimits,
  projectView,
  type ProjectFile,
} from "./engine.ts";
import { errorMessage } from "./errors.ts";
import { GitHub, GitHubError, type Commit, type Refs, type Tree } from "./github.ts";
import type { BaseSide, Compare, EditResult, Host, NoteKind } from "./host.ts";
import type { TokenStore } from "./login.ts";
import type { ViewModel } from "./model.ts";
import { ModelCache } from "./modelCache.ts";
import { acceptEdit, projectionNote, type ViewProjection } from "./projection.ts";
import { RefsCache } from "./refsCache.ts";
import { place, recorded, type Revised, type SourceEdit } from "./revisions.ts";
import { requestOf, Sources } from "./sources.ts";
import {
  join,
  locateProject,
  planProject,
  ProjectPlanError,
  type ProjectPlan,
  type ProjectShape,
} from "./repoProject.ts";
import {
  baseRef,
  formatRoute,
  headRef,
  parseRoute,
  sameRef,
  withHead,
  type Route,
} from "./route.ts";

/** What the shell shows about the host: the address being read and whether it is busy. */
export interface GitHubState {
  route: Route;
  /** The ref GitHub resolved the route to (the default branch, when the route names none). */
  ref: string | null;
  loading: boolean;
  /** Whether the current GitHub failure asks for a token. */
  wantsToken: boolean;
}

/** Fetch at most `limit` at a time. */
async function mapLimit<T, R>(
  items: readonly T[],
  limit: number,
  fn: (item: T) => Promise<R>,
): Promise<R[]> {
  const out: R[] = [];
  let next = 0;
  const worker = async (): Promise<void> => {
    while (next < items.length) {
      const i = next++;
      out[i] = await fn(items[i] as T);
    }
  };
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker));
  return out;
}

/**
 * The project around `path` in `tree`, planned within the engine's limits
 * before its sources are read with `read`. A package's manifest is read
 * first, for where its modules live, and not read again.
 */
async function readProject(
  tree: Tree,
  path: string,
  read: (path: string) => Promise<string>,
): Promise<{ plan: ProjectPlan; files: [string, string][] }> {
  const location = locateProject(tree.entries, path);
  const limits = await projectLimits();
  let shape: ProjectShape;
  let manifest: { path: string; text: string } | null = null;
  if (location.kind === "package") {
    manifest = { path: location.manifest, text: await read(location.manifest) };
    const layout = await packageLayout(manifest.text);
    if ("error" in layout) throw new ProjectPlanError(`${location.manifest}: ${layout.error}`);
    shape = { ...location, layout: layout.layout };
  } else {
    shape = location;
  }
  const plan = planProject(tree.entries, shape, limits);
  const texts = await mapLimit(plan.files, 6, (file) =>
    manifest !== null && file.path === manifest.path
      ? Promise.resolve(manifest.text)
      : read(file.path),
  );
  return {
    plan,
    files: plan.files.map((file, i) => [file.relative, texts[i] ?? ""] as [string, string]),
  };
}

/** `promise`'s outcome, which never rejects (so it may wait unobserved). */
function settle<T>(promise: Promise<T>): Promise<PromiseSettledResult<T>> {
  return promise.then(
    (value) => ({ status: "fulfilled", value }),
    (reason: unknown) => ({ status: "rejected", reason }),
  );
}

/** Reads a file of `tree` by its blob's hash, so a push since cannot change what is read. */
function blobReader(github: GitHub, route: Route, tree: Tree): (path: string) => Promise<string> {
  const shas = new Map(tree.entries.filter((e) => e.type === "blob").map((e) => [e.path, e.sha]));
  return (path) => {
    const sha = shas.get(path);
    return sha === undefined
      ? Promise.reject(new Error(`${path} is not in the tree`))
      : github.blob(route.owner, route.repo, sha);
  };
}

/** The base model of one project file, or why it has none. */
async function baseModel(file: string, request: ProjectFile[]): Promise<BaseSide<ViewModel>> {
  try {
    const projection = await projectView(file, request);
    // A library waiting for its includers' values still has its structure to compare.
    if (projection.status === "invalid") return { kind: "failed", message: projection.message };
    return { kind: "present", value: projection.view };
  } catch (error) {
    return { kind: "failed", message: errorMessage(error) };
  }
}

/**
 * The project at the base commit: the same files, read and projected there.
 * The file may not exist at the base (a new file): then every node is new.
 * A blob the head holds too is not fetched again. Never rejects: a base that
 * cannot be read is a failed base, and the head stays shown.
 */
async function readBase(
  github: GitHub,
  route: Route,
  commit: PromiseSettledResult<Commit>,
  headTree: Tree,
): Promise<BaseRead> {
  try {
    if (commit.status === "rejected") throw commit.reason;
    const baseTree = await github.tree(route.owner, route.repo, commit.value.treeSha);
    if (baseTree.truncated) {
      return {
        kind: "failed",
        message: "the repository is too large for GitHub to list in one request",
      };
    }
    const changedFiles = changedPaths(headTree.entries, baseTree.entries);
    const sources = new Map<string, string>();
    const models = new Map<string, BaseSide<ViewModel>>();
    const exists = baseTree.entries.some((e) => e.type === "blob" && e.path === route.path);
    if (exists) {
      const { plan, files } = await readProject(
        baseTree,
        route.path,
        blobReader(github, route, baseTree),
      );
      for (const [file, text] of files) sources.set(file, text);
      const request = requestOf(sources);
      for (const file of plan.files) {
        if (file.relative.endsWith(".gcl")) {
          models.set(file.relative, await baseModel(file.relative, request));
        }
      }
    }
    return { kind: "read", changedFiles, sources, models };
  } catch (error) {
    return { kind: "failed", message: errorMessage(error) };
  }
}

export class GitHubHost implements Host {
  modules: readonly string[] = [];
  /** Every file of the repository at the ref (for the explorer). */
  files: readonly string[] = [];
  /** The base side, once read, when the address compares two refs. */
  compare: Compare | null = null;
  edited = false;
  private route: Route;
  private github: GitHub;
  private sources: Sources | null = null;
  /** The revision of the sources; every edit, load and reset makes a new one. */
  private revision = 0;
  /** The sources as read from GitHub, to return to on reset. */
  private pristine = new Map<string, string>();
  private readonly models = new ModelCache();
  private readonly refsCache = new RefsCache();
  private pristineModels: ViewModel[] = [];
  /** Repository directory of the project root, to turn a project file into an address. */
  private root = "";
  private state: GitHubState;
  /** Bumped per load so a stale load does not overwrite a newer one. */
  private generation = 0;
  private readonly modelListeners: ((models: ViewModel[]) => void)[] = [];
  private readonly openListeners: ((module: string) => void)[] = [];
  private readonly noteListeners: ((message: string | null, kind: NoteKind) => void)[] = [];
  private readonly stateListeners: ((state: GitHubState) => void)[] = [];

  private readonly tokens: TokenStore;

  constructor(route: Route, tokens: TokenStore) {
    this.route = route;
    this.tokens = tokens;
    this.github = new GitHub(tokens.get());
    this.state = { route, ref: headRef(route.at), loading: false, wantsToken: false };
    tokens.onChange(() => {
      this.github = new GitHub(tokens.get());
      void this.load();
    });
    window.addEventListener("hashchange", () => this.followHash());
    void loadEngine().catch(() => undefined);
    void this.load();
  }

  // ---- loading ----

  /** Read the project at the route and show its entry file. */
  private async load(): Promise<void> {
    const generation = ++this.generation;
    const fresh = (): boolean => generation === this.generation;
    const { route } = this;
    this.setState({ route, ref: headRef(route.at), loading: true, wantsToken: false });
    this.note(`reading ${route.owner}/${route.repo}…`, "busy");
    try {
      const github = this.github;
      const info = await github.repo(route.owner, route.repo);
      const ref = headRef(route.at) ?? info.defaultBranch;
      if (!fresh()) return;
      this.setState({ ...this.state, ref });
      // Both refs are resolved to commits first; everything after is read by hash.
      const base = baseRef(route.at);
      const baseCommit =
        base === null ? null : settle(github.commit(route.owner, route.repo, base));
      const head = await github.commit(route.owner, route.repo, ref);
      const tree = await github.tree(route.owner, route.repo, head.treeSha);
      if (!fresh()) return;
      if (tree.truncated) {
        throw new Error("the repository is too large for GitHub to list in one request");
      }
      this.files = tree.entries.filter((e) => e.type === "blob").map((e) => e.path);
      this.compare = null;
      const { plan, files } = await readProject(tree, route.path, blobReader(github, route, tree));
      if (!fresh()) return;
      this.root = plan.root;
      this.pristine = new Map(files);
      this.sources = new Sources(files, ++this.revision);
      this.models.replace([], this.revision);
      this.modules = [];
      this.edited = false;
      this.note("evaluating…", "busy");
      // The entry first, so the sheet appears; the other files follow for Open / Pin.
      const entry = await this.projectFile(plan.entry);
      if (!fresh()) return;
      if ("error" in entry) {
        this.note(entry.error, "error");
      } else {
        this.note(entry.note, "error");
        for (const listener of this.openListeners) listener(entry.model.module.id);
      }
      for (const file of plan.files) {
        if (file.relative === plan.entry || !file.relative.endsWith(".gcl")) continue;
        const outcome = await this.projectFile(file.relative);
        if (!fresh()) return;
        if ("error" in outcome) continue;
        for (const listener of this.modelListeners) listener([outcome.model]);
      }
      this.pristineModels = this.models.currentModels();
      if (base !== null && baseCommit !== null) {
        this.note(`reading ${base}…`, "busy");
        const read = await readBase(github, route, await baseCommit, tree);
        // Published only by the load that is still current.
        if (!fresh()) return;
        this.compare = compareOf(base, read);
        if (read.kind === "failed") {
          this.note(`cannot compare with ${base}: ${read.message}`, "error");
        } else {
          this.note(null, "busy");
        }
        // The sheets are drawn again with the changes marked.
        for (const listener of this.modelListeners) listener(this.models.currentModels());
      }
      this.setState({ ...this.state, loading: false });
    } catch (error) {
      if (!fresh()) return;
      this.fail(error);
    }
  }

  /** Project one file of the project and remember its model. */
  private async projectFile(
    file: string,
  ): Promise<{ model: ViewModel; note: string | null } | { error: string }> {
    for (;;) {
      const sources = this.sources;
      if (!sources) return { error: "no project" };
      const revision = sources.revision;
      let projection: ViewProjection;
      try {
        projection = await projectView(file, sources.request());
      } catch (error) {
        return { error: errorMessage(error) };
      }
      if (projection.status === "invalid") return { error: `${file}: ${projection.message}` };
      const model = projection.view;
      // An edit or a reset while it ran: what it read is gone, so it runs again.
      if (!this.models.set(model, revision)) continue;
      this.modules = this.models.ids();
      return { model, note: projectionNote(file, projection) };
    }
  }

  private fail(error: unknown): void {
    const { route } = this;
    let message = errorMessage(error);
    let wantsToken = false;
    if (error instanceof GitHubError) {
      const signedIn = this.tokens.get() !== null;
      switch (error.kind) {
        case "not_found":
          message = signedIn
            ? `${route.owner}/${route.repo} has no ${route.path} at this ref, or the token cannot read it`
            : `${route.owner}/${route.repo} was not found; if it is private, sign in with a token`;
          wantsToken = !signedIn;
          break;
        case "ref_not_found":
          message = error.message;
          break;
        case "unauthorized":
          message = "GitHub rejected the token; paste a new one";
          wantsToken = true;
          break;
        case "rate_limited":
          message = signedIn ? error.message : `${error.message}; a token raises the limit`;
          wantsToken = !signedIn;
          break;
        default:
          break;
      }
    } else if (error instanceof ProjectPlanError) {
      message = error.message;
    }
    this.note(message, "error");
    this.setState({ ...this.state, loading: false, wantsToken });
  }

  /** The address changed by hand or by history: same project, just show the file; else reload. */
  private followHash(): void {
    const next = parseRoute(location.hash);
    if (!next) return;
    const same =
      next.owner === this.route.owner &&
      next.repo === this.route.repo &&
      sameRef(next.at, this.route.at);
    this.route = next;
    if (same && this.sources) {
      const module = this.moduleOfPath(next.path);
      if (module !== null) {
        void this.openModule(module);
        return;
      }
    }
    void this.load();
  }

  /** A `.gcl` of the repository: shown when it is in the project read, else read as a new project. */
  openFile(path: string): boolean {
    if (!path.endsWith(".gcl")) return false;
    const module = this.moduleOfPath(path);
    if (module !== null) {
      void this.openModule(module);
      return true;
    }
    location.hash = formatRoute({ ...this.route, path });
    return true;
  }

  /** The edits since the project was read, as a unified diff with repository paths. */
  patch(): Patch {
    const sources = this.sources;
    if (!sources) return { text: "", files: 0 };
    const changes = changedFiles(this.pristine, sources.texts(), (file) => join(this.root, file));
    const ref = this.state.ref ?? headRef(this.route.at) ?? "default branch";
    return patchOf(changes, `graphcal what-if: ${this.route.owner}/${this.route.repo} @ ${ref}`);
  }

  pathOf(module: string): string | null {
    const model = this.models.last(module);
    if (!model) return null;
    return join(this.root, model.module.file);
  }

  private moduleOfPath(path: string): string | null {
    if (this.root !== "" && !path.startsWith(`${this.root}/`)) return null;
    const relative = this.root === "" ? path : path.slice(this.root.length + 1);
    return this.models.moduleOfFile(relative);
  }

  /** The current model of `module`, projected again when an edit left it stale. */
  private async currentModel(module: string): Promise<ViewModel | null> {
    const current = this.models.current(module);
    if (current) return current;
    const last = this.models.last(module);
    if (!last) return null;
    const generation = this.generation;
    const outcome = await this.projectFile(last.module.file);
    if (generation !== this.generation) return null;
    if ("error" in outcome) {
      this.note(outcome.error, "error");
      return null;
    }
    return outcome.model;
  }

  /** Ask the shell to show `module`, as of the current sources. */
  private async openModule(module: string): Promise<void> {
    if (!(await this.currentModel(module))) return;
    for (const listener of this.openListeners) listener(module);
  }

  // ---- refs ----

  /** The branches, tags and pull requests of the repository being read. */
  refs(): Promise<Refs> {
    const { owner, repo } = this.route;
    const github = this.github;
    return this.refsCache.get({ owner, repo, token: this.tokens.get() }, () =>
      github.refs(owner, repo),
    );
  }

  /** Read the same file at another ref: the address changes and the project is read again. */
  switchRef(ref: string): void {
    location.hash = formatRoute({ ...this.route, at: withHead(this.route.at, ref) });
  }

  // ---- shell events ----

  onState(listener: (state: GitHubState) => void): void {
    this.stateListeners.push(listener);
    listener(this.state);
  }

  private setState(state: GitHubState): void {
    this.state = state;
    for (const listener of this.stateListeners) listener(state);
  }

  private note(message: string | null, kind: NoteKind): void {
    for (const listener of this.noteListeners) listener(message, kind);
  }

  // ---- Host ----

  model(module: string): ViewModel | null {
    return this.models.current(module);
  }

  source(_model: ViewModel, file: string): Revised | null {
    return this.sources?.source(file) ?? null;
  }

  projectedAt(model: ViewModel): number | null {
    return this.models.revisionOf(model);
  }

  /**
   * Place the edit on the latest text of its file, through the edits made
   * since its revision, and keep it when the sources still check.
   */
  async edit(_model: ViewModel, edit: SourceEdit, open: readonly string[]): Promise<EditResult> {
    const sources = this.sources;
    const history = sources?.history(edit.file) ?? null;
    if (!sources || history === null) return { ok: false, message: `unknown file ${edit.file}` };
    const placed = place(history, edit);
    if (!placed.ok) return placed;
    const at = this.revision;
    const next = recorded(history, placed.change, at + 1);
    const request = sources.request({ file: edit.file, history: next });
    const projections: { file: string; projection: ViewProjection }[] = [];
    for (const module of open) {
      const shown = this.models.current(module);
      if (!shown) continue;
      const file = shown.module.file;
      try {
        projections.push({ file, projection: await projectView(file, request) });
      } catch (error) {
        return { ok: false, message: errorMessage(error) };
      }
    }
    // A load or a reset while it ran: the edit was made on sources that are gone.
    if (this.sources !== sources || this.revision !== at) {
      return { ok: false, message: "the sources changed while the edit ran; edit again" };
    }
    // An edit stands while the sources check, whether or not they evaluate.
    const verdict = acceptEdit(projections);
    if (!verdict.ok) return verdict;
    this.revision = next.revision;
    sources.commit(edit.file, next);
    // The modules off screen read the edited sources too: their models are stale now.
    this.models.edited(verdict.models, next.revision);
    this.edited = true;
    return { ...verdict, source: { text: next.text, revision: next.revision } };
  }

  request(module: string): void {
    void this.currentModel(module).then((model) => {
      if (model) for (const listener of this.modelListeners) listener([model]);
    });
  }

  reset(): void {
    if (!this.sources) return;
    this.sources = new Sources(this.pristine, ++this.revision);
    this.models.replace(this.pristineModels, this.revision);
    this.edited = false;
  }

  shown(module: string): void {
    const path = this.pathOf(module);
    if (path === null || path === this.route.path) return;
    this.route = { ...this.route, path };
    history.replaceState(null, "", formatRoute(this.route));
    this.setState({ ...this.state, route: this.route });
  }

  onModels(listener: (models: ViewModel[]) => void): void {
    this.modelListeners.push(listener);
  }

  onOpen(listener: (module: string) => void): void {
    this.openListeners.push(listener);
  }

  onNote(listener: (message: string | null, kind: NoteKind) => void): void {
    this.noteListeners.push(listener);
  }
}
