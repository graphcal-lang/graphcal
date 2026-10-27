// The web viewer's host: the project around a `.gcl` file is read from GitHub
// through the browser extension, kept in memory, and projected by the
// in-browser engine. Nothing is written back; edits are a what-if over the
// sources read, and `reset` returns to them. The address bar follows the
// shown file.
import { changedFiles, patchOf, type Patch } from "./diff.ts";
import { compareOf, type BaseRead } from "./compareBase.ts";
import {
  loadEngine,
  packageLayout,
  projectLimits,
  projectView,
  type EngineTarget,
  type ProjectFile,
} from "./engine.ts";
import { errorMessage } from "./errors.ts";
import { GitHubError, type Commit, type GitHub, type Refs, type Tree } from "./github.ts";
import type { BaseSide, Compare, EditResult, Host, NoteKind } from "./host.ts";
import type { ViewModel } from "./model.ts";
import { ModelCache } from "./modelCache.ts";
import { acceptEdit, projectionNote, type ViewProjection } from "./projection.ts";
import { RefsCache } from "./refsCache.ts";
import { place, recorded, type Revised, type SourceEdit } from "./revisions.ts";
import { requestOf, Sources } from "./sources.ts";
import { moduleTarget, TargetMap, targetOf, type ViewTarget } from "./target.ts";
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
  /** Whether the current failure is that the browser has no Graphcal extension. */
  needsExtension: boolean;
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
 * before its sources are read with `read`, then planned again with the sizes
 * of what was read (the listing gives none), so files beside the entry that
 * are too big are left out instead of failing the load. A package's manifest
 * is read first, for where its modules live, and not read again.
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
  const initial = planProject(tree.entries, shape, limits);
  const texts = await mapLimit(initial.files, 6, (file) =>
    manifest !== null && file.path === manifest.path
      ? Promise.resolve(manifest.text)
      : read(file.path),
  );
  const textByPath = new Map(initial.files.map((file, i) => [file.path, texts[i] ?? ""]));
  const encoder = new TextEncoder();
  const measured = tree.entries.map((entry) => {
    const text = textByPath.get(entry.path);
    return text === undefined ? entry : { ...entry, size: encoder.encode(text).length };
  });
  const plan = planProject(measured, shape, limits);
  return {
    plan,
    files: plan.files.map((file) => [file.relative, textByPath.get(file.path) ?? ""]),
  };
}

/** `promise`'s outcome, which never rejects (so it may wait unobserved). */
function settle<T>(promise: Promise<T>): Promise<PromiseSettledResult<T>> {
  return promise.then(
    (value) => ({ status: "fulfilled", value }),
    (reason: unknown) => ({ status: "rejected", reason }),
  );
}

/** Reads a file of the repository at `commit`, so a push since cannot change what is read. */
function fileReader(
  github: GitHub,
  route: Route,
  commit: Commit,
): (path: string) => Promise<string> {
  return (path) => github.read(route.owner, route.repo, commit, path);
}

/** The base model of one project file, or why it has none. */
async function baseModel(file: string, request: ProjectFile[]): Promise<BaseSide<ViewModel>> {
  try {
    const projection = await projectView(file, request);
    // A library waiting for its includers' values still has its structure to compare.
    if (projection.status === "invalid" || projection.status === "gone") {
      return { kind: "failed", message: projection.message };
    }
    return { kind: "present", value: projection.view };
  } catch (error) {
    return { kind: "failed", message: errorMessage(error) };
  }
}

/**
 * The project at the base commit: the same files, read and projected there.
 * The file may not exist at the base (a new file): then every node is new.
 * What changed between the commits is GitHub's compare (what a pull request
 * from `head` would change). Never rejects: a base that cannot be read is a
 * failed base, and the head stays shown.
 */
async function readBase(
  github: GitHub,
  route: Route,
  commit: PromiseSettledResult<Commit>,
  head: Commit,
): Promise<BaseRead> {
  try {
    if (commit.status === "rejected") throw commit.reason;
    const base = commit.value;
    const baseTree = await github.tree(route.owner, route.repo, base);
    const changedFiles = await github.changed(route.owner, route.repo, base.sha, head.sha);
    const sources = new Map<string, string>();
    const models = new Map<string, BaseSide<ViewModel>>();
    const exists = baseTree.entries.some((e) => e.path === route.path);
    if (exists) {
      const { plan, files } = await readProject(
        baseTree,
        route.path,
        fileReader(github, route, base),
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

/** What one successful read of a project gave: kept together so they never fall out of step. */
interface Loaded {
  /** The address the project was read at. */
  route: Route;
  /** Repository directory of the project root, to turn a project file into an address. */
  root: string;
  /** The sources as read from GitHub, to return to on reset. */
  pristine: ReadonlyMap<string, string>;
  /** The revision at which the sources are as read (the load's, or the last reset's). */
  pristineRevision: number;
  /** The models of the sources as read, by target, as they are projected. */
  pristineModels: TargetMap<ViewModel>;
}

export class GitHubHost implements Host {
  modules: readonly string[] = [];
  /** Every file of the repository at the ref (for the explorer). */
  files: readonly string[] = [];
  /** The base side, once read, when the address compares two refs. */
  compare: Compare | null = null;
  edited = false;
  private route: Route;
  /** GitHub through the extension, once it has answered; a load waits for it. */
  private readonly connection: Promise<GitHub>;
  private sources: Sources | null = null;
  /** The revision of the sources; every edit, load and reset makes a new one. */
  private revision = 0;
  /** The project read last; a load that fails leaves it, and the address, as they were. */
  private loaded: Loaded | null = null;
  private readonly models = new ModelCache();
  private readonly refsCache = new RefsCache();
  private state: GitHubState;
  /** Bumped per load so a stale load does not overwrite a newer one. */
  private generation = 0;
  private readonly modelListeners: ((models: ViewModel[]) => void)[] = [];
  private readonly openListeners: ((module: string) => void)[] = [];
  private readonly noteListeners: ((message: string | null, kind: NoteKind) => void)[] = [];
  private readonly stateListeners: ((state: GitHubState) => void)[] = [];

  constructor(route: Route, connection: Promise<GitHub>) {
    this.route = route;
    this.connection = connection;
    this.state = { route, ref: headRef(route.at), loading: false, needsExtension: false };
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
    this.setState({ route, ref: headRef(route.at), loading: true, needsExtension: false });
    this.note(`reading ${route.owner}/${route.repo}…`, "busy");
    try {
      const github = await this.connection;
      const info = await github.repo(route.owner, route.repo);
      const ref = headRef(route.at) ?? info.defaultBranch;
      if (!fresh()) return;
      this.setState({ ...this.state, ref });
      // Both refs are resolved to commits first; everything after is read by hash.
      const base = baseRef(route.at);
      const baseCommit =
        base === null ? null : settle(github.commit(route.owner, route.repo, base));
      const head = await github.commit(route.owner, route.repo, ref);
      const tree = await github.tree(route.owner, route.repo, head);
      if (!fresh()) return;
      this.files = tree.entries.map((e) => e.path);
      this.compare = null;
      const { plan, files } = await readProject(tree, route.path, fileReader(github, route, head));
      if (!fresh()) return;
      this.sources = new Sources(files, ++this.revision);
      this.loaded = {
        route,
        root: plan.root,
        pristine: new Map(files),
        pristineRevision: this.revision,
        pristineModels: new TargetMap(),
      };
      this.models.replace([], this.revision);
      this.modules = [];
      this.edited = false;
      this.note("evaluating…", "busy");
      // The entry first, so the sheet appears; the other files follow for Open / Pin.
      const entry = await this.projectEntry(plan.entry, { kind: "module" });
      if (!fresh()) return;
      if ("error" in entry) {
        this.note(entry.error, "error");
      } else {
        this.note(entry.note, "error");
        for (const listener of this.openListeners) listener(entry.model.module.id);
      }
      for (const file of plan.files) {
        if (file.relative === plan.entry || !file.relative.endsWith(".gcl")) continue;
        const outcome = await this.projectEntry(file.relative, { kind: "module" });
        if (!fresh()) return;
        if ("error" in outcome) continue;
        for (const listener of this.modelListeners) listener([outcome.model]);
      }
      if (base !== null && baseCommit !== null) {
        this.note(`reading ${base}…`, "busy");
        const read = await readBase(github, route, await baseCommit, head);
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

  /** Project `target` of one file of the project (its own view, or an instance) and remember the model. */
  private async projectEntry(
    file: string,
    target: EngineTarget,
  ): Promise<{ model: ViewModel; note: string | null } | { error: string }> {
    for (;;) {
      const sources = this.sources;
      if (!sources) return { error: "no project" };
      const revision = sources.revision;
      let projection: ViewProjection;
      try {
        projection = await projectView(file, sources.request(), target);
      } catch (error) {
        return { error: errorMessage(error) };
      }
      if (projection.status === "invalid" || projection.status === "gone") {
        return { error: `${file}: ${projection.message}` };
      }
      const model = projection.view;
      // An edit or a reset while it ran: what it read is gone, so it runs again.
      if (!this.models.set(model, revision)) continue;
      this.modules = this.models.moduleIds();
      const loaded = this.loaded;
      if (loaded !== null && revision === loaded.pristineRevision) {
        loaded.pristineModels.set(targetOf(model), model);
      }
      return { model, note: projectionNote(file, projection) };
    }
  }

  /**
   * The file the engine projects `target` from, and what of it: a module's
   * own file, or for an include instance its root's file. Null while the
   * host has no model that locates it.
   */
  private entryOf(target: ViewTarget): { file: string; engine: EngineTarget } | null {
    if (target.kind === "module") {
      const model = this.models.last(target);
      return model ? { file: model.module.file, engine: { kind: "module" } } : null;
    }
    const root = this.models.last(moduleTarget(target.root));
    return root
      ? { file: root.module.file, engine: { kind: "instance", instance: target.instance } }
      : null;
  }

  private fail(error: unknown): void {
    const { route } = this;
    let message = errorMessage(error);
    let needsExtension = false;
    if (error instanceof GitHubError) {
      switch (error.kind) {
        case "no_extension":
          message =
            "the Graphcal extension is not installed in this browser; install it (top right) and reload";
          needsExtension = true;
          break;
        case "not_found":
          message = `${route.owner}/${route.repo} has no ${route.path} at this ref, or your GitHub account cannot see it`;
          break;
        default:
          message = error.message;
          break;
      }
    } else if (error instanceof ProjectPlanError) {
      message = error.message;
    }
    this.note(message, "error");
    this.setState({ ...this.state, loading: false, needsExtension });
  }

  /** The address changed by hand or by history: same project, just show the file; else reload. */
  private followHash(): void {
    const next = parseRoute(location.hash);
    if (!next) return;
    const read = this.loaded?.route ?? null;
    const same =
      read !== null &&
      next.owner === read.owner &&
      next.repo === read.repo &&
      sameRef(next.at, read.at);
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
    location.hash = formatRoute({ ...(this.loaded?.route ?? this.route), path });
    return true;
  }

  /** The edits since the project was read, as a unified diff with repository paths. */
  patch(): Patch {
    const { sources, loaded } = this;
    if (!sources || !loaded) return { text: "", files: 0 };
    const changes = changedFiles(loaded.pristine, sources.texts(), (file) =>
      join(loaded.root, file),
    );
    const { owner, repo, at } = loaded.route;
    const ref = this.state.ref ?? headRef(at) ?? "default branch";
    return patchOf(changes, `graphcal what-if: ${owner}/${repo} @ ${ref}`);
  }

  pathOf(module: string): string | null {
    const model = this.models.last(moduleTarget(module));
    if (!model || !this.loaded) return null;
    return join(this.loaded.root, model.module.file);
  }

  private moduleOfPath(path: string): string | null {
    const root = this.loaded?.root ?? null;
    if (root === null || (root !== "" && !path.startsWith(`${root}/`))) return null;
    const relative = root === "" ? path : path.slice(root.length + 1);
    return this.models.moduleOfFile(relative);
  }

  /**
   * The current model of `target`, projected when the host has none yet (an
   * include instance pinned for the first time) or an edit left it stale.
   */
  private async currentModel(target: ViewTarget): Promise<ViewModel | null> {
    const current = this.models.current(target);
    if (current) return current;
    const entry = this.entryOf(target);
    if (!entry) return null;
    const generation = this.generation;
    const outcome = await this.projectEntry(entry.file, entry.engine);
    if (generation !== this.generation) return null;
    if ("error" in outcome) {
      this.note(outcome.error, "error");
      return null;
    }
    return outcome.model;
  }

  /** Ask the shell to show `module`, as of the current sources. */
  private async openModule(module: string): Promise<void> {
    if (!(await this.currentModel(moduleTarget(module)))) return;
    for (const listener of this.openListeners) listener(module);
  }

  // ---- refs ----

  /** The branches, tags and pull requests of the repository being read. */
  refs(): Promise<Refs> {
    const { owner, repo } = this.route;
    return this.refsCache.get({ owner, repo }, async () =>
      (await this.connection).refs(owner, repo),
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

  model(target: ViewTarget): ViewModel | null {
    return this.models.current(target);
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
  async edit(
    _model: ViewModel,
    edit: SourceEdit,
    open: readonly ViewTarget[],
  ): Promise<EditResult> {
    const sources = this.sources;
    const history = sources?.history(edit.file) ?? null;
    if (!sources || history === null) return { ok: false, message: `unknown file ${edit.file}` };
    const placed = place(history, edit);
    if (!placed.ok) return placed;
    const at = this.revision;
    const next = recorded(history, placed.change, at + 1);
    const request = sources.request({ file: edit.file, history: next });
    const projections: { file: string; projection: ViewProjection }[] = [];
    for (const target of open) {
      const entry = this.models.current(target) ? this.entryOf(target) : null;
      if (!entry) continue;
      const { file, engine } = entry;
      try {
        projections.push({ file, projection: await projectView(file, request, engine) });
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
    // The targets off screen read the edited sources too: their models are stale now.
    this.models.edited(verdict.models, next.revision);
    this.edited = true;
    return { ...verdict, source: { text: next.text, revision: next.revision } };
  }

  request(target: ViewTarget): void {
    void this.currentModel(target).then((model) => {
      if (model) for (const listener of this.modelListeners) listener([model]);
    });
  }

  /**
   * Back to the sources as read, at a new revision: an edit still running
   * finds the revision moved and is refused, and a projection of the edited
   * sources finishing later is not kept.
   */
  reset(): void {
    const loaded = this.loaded;
    if (!this.sources || !loaded) return;
    this.sources = new Sources(loaded.pristine, ++this.revision);
    loaded.pristineRevision = this.revision;
    this.models.replace(
      loaded.pristineModels.entries().map(([, model]) => model),
      this.revision,
    );
    this.modules = this.models.moduleIds();
    this.edited = false;
  }

  shown(module: string): void {
    const path = this.pathOf(module);
    const read = this.loaded?.route;
    if (path === null || read === undefined) return;
    const { route } = this;
    const unchanged =
      route.owner === read.owner &&
      route.repo === read.repo &&
      sameRef(route.at, read.at) &&
      route.path === path;
    if (unchanged) return;
    this.route = { ...read, path };
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
