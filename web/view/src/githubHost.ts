// The web viewer's host: the project around a `.gcl` file is read from GitHub
// with the viewer's own token, kept in memory, and projected by the in-browser
// engine. Nothing is written back; edits are a what-if over the sources read,
// and `reset` returns to them. The address bar follows the shown file.
import { evaluateView, loadEngine, viewOf } from "./engine.ts";
import { GitHub, GitHubError } from "./github.ts";
import type { EditResult, Host, NoteKind, SourceEdit } from "./host.ts";
import type { TokenStore } from "./login.ts";
import { parseViewModel, type ViewModel } from "./model.ts";
import { Project } from "./project.ts";
import { planProject, ProjectPlanError } from "./repoProject.ts";
import { formatRoute, parseRoute, type Route } from "./route.ts";

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

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export class GitHubHost implements Host {
  modules: readonly string[] = [];
  edited = false;
  private route: Route;
  private github: GitHub;
  private project: Project | null = null;
  /** The sources as read from GitHub, to return to on reset. */
  private pristine = new Map<string, string>();
  private readonly models = new Map<string, ViewModel>();
  private pristineModels = new Map<string, ViewModel>();
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
    this.state = { route, ref: route.ref, loading: false, wantsToken: false };
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
    this.setState({ route, ref: route.ref, loading: true, wantsToken: false });
    this.note(`reading ${route.owner}/${route.repo}…`, "busy");
    try {
      const info = await this.github.repo(route.owner, route.repo);
      const ref = route.ref ?? info.defaultBranch;
      if (!fresh()) return;
      this.setState({ ...this.state, ref });
      const tree = await this.github.tree(route.owner, route.repo, ref);
      if (!fresh()) return;
      if (tree.truncated) {
        throw new Error("the repository is too large for GitHub to list in one request");
      }
      const plan = planProject(tree.entries, route.path);
      const texts = await mapLimit(plan.files, 6, (file) =>
        this.github.file(route.owner, route.repo, ref, file.path),
      );
      if (!fresh()) return;
      const files = plan.files.map(
        (file, i) => [file.relative, texts[i] ?? ""] as [string, string],
      );
      this.root = plan.root;
      this.pristine = new Map(files);
      this.project = new Project(route.repo, files);
      this.models.clear();
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
      this.pristineModels = new Map(this.models);
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
    if (!this.project) return { error: "no project" };
    let outcome;
    try {
      outcome = await viewOf(file, this.project.request());
    } catch (error) {
      return { error: errorMessage(error) };
    }
    if ("error" in outcome) return { error: `${file}: ${outcome.error}` };
    let model: ViewModel;
    try {
      model = parseViewModel(outcome.view);
    } catch (error) {
      return { error: errorMessage(error) };
    }
    this.models.set(model.module.id, model);
    this.modules = [...this.models.keys()].sort();
    return {
      model,
      note: outcome.valued ? null : `${file} has no values of its own: ${outcome.note}`,
    };
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
      next.ref === this.route.ref;
    this.route = next;
    if (same && this.project) {
      const module = this.moduleOfPath(next.path);
      if (module !== null) {
        for (const listener of this.openListeners) listener(module);
        return;
      }
    }
    void this.load();
  }

  private moduleOfPath(path: string): string | null {
    const relative = this.root === "" ? path : path.slice(this.root.length + 1);
    for (const model of this.models.values()) {
      if (model.module.file === relative) return model.module.id;
    }
    return null;
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
    return this.models.get(module) ?? null;
  }

  source(_model: ViewModel, file: string): string | null {
    return this.project?.files.get(file) ?? null;
  }

  async edit(_model: ViewModel, edit: SourceEdit, open: readonly string[]): Promise<EditResult> {
    const project = this.project;
    const previous = project?.files.get(edit.file);
    if (!project || previous === undefined)
      return { ok: false, message: `unknown file ${edit.file}` };
    project.files.set(edit.file, edit.next);
    const rollback = (message: string): EditResult => {
      project.files.set(edit.file, previous);
      return { ok: false, message };
    };
    const models: ViewModel[] = [];
    for (const module of open) {
      const shown = this.models.get(module);
      if (!shown) continue;
      let outcome;
      try {
        outcome = await evaluateView(shown.module.file, project.request());
      } catch (error) {
        return rollback(errorMessage(error));
      }
      if ("error" in outcome) return rollback(outcome.error);
      try {
        models.push(parseViewModel(outcome.view));
      } catch (error) {
        return rollback(errorMessage(error));
      }
    }
    for (const fresh of models) this.models.set(fresh.module.id, fresh);
    this.edited = true;
    return { ok: true, models };
  }

  request(module: string): void {
    const model = this.models.get(module);
    if (model) for (const listener of this.modelListeners) listener([model]);
  }

  reset(): void {
    if (!this.project) return;
    this.project = new Project(this.route.repo, this.pristine);
    this.models.clear();
    for (const [id, model] of this.pristineModels) this.models.set(id, model);
    this.edited = false;
  }

  shown(module: string): void {
    const model = this.models.get(module);
    if (!model) return;
    const path = this.root === "" ? model.module.file : `${this.root}/${model.module.file}`;
    if (path === this.route.path) return;
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
