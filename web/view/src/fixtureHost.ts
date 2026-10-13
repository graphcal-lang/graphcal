// The dev shell's host: models from the bundled fixtures, edits applied to an
// in-memory copy of the fixture project's sources and re-projected by the
// in-browser engine. Nothing is written to disk; `reset` reloads the fixtures.
import { evaluateView, loadEngine } from "./engine.ts";
import type { EditResult, Host, SourceEdit } from "./host.ts";
import { parseViewModel, type ViewModel } from "./model.ts";
import { projectFor, type Project } from "./project.ts";

const fixtures = import.meta.glob<unknown>("../fixtures/*.view.json", {
  eager: true,
  import: "default",
});

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export class FixtureHost implements Host {
  readonly modules: readonly string[];
  edited = false;
  private readonly models = new Map<string, ViewModel>();
  /** One in-memory project per package, shared by every sheet of that package. */
  private readonly projects = new Map<string, Project>();
  private readonly modelListeners: ((models: ViewModel[]) => void)[] = [];
  private readonly openListeners: ((module: string) => void)[] = [];

  constructor() {
    for (const raw of Object.values(fixtures)) {
      const model = parseViewModel(raw);
      this.models.set(model.module.id, model);
    }
    this.modules = [...this.models.keys()].sort();
    // Warm the engine so the first edit does not wait for the download.
    void loadEngine().catch(() => undefined);
  }

  model(module: string): ViewModel | null {
    return this.models.get(module) ?? null;
  }

  private projectOf(model: ViewModel): Project | null {
    const cached = this.projects.get(model.module.package);
    if (cached) return cached;
    const project = projectFor(model);
    if (project) this.projects.set(model.module.package, project);
    return project;
  }

  source(model: ViewModel, file: string): string | null {
    return this.projectOf(model)?.files.get(file) ?? null;
  }

  async edit(model: ViewModel, edit: SourceEdit, open: readonly string[]): Promise<EditResult> {
    const project = this.projectOf(model);
    const previous = project?.files.get(edit.file);
    if (!project || previous === undefined)
      return { ok: false, message: `unknown file ${edit.file}` };
    project.files.set(edit.file, edit.next);
    const rollback = (message: string): EditResult => {
      project.files.set(edit.file, previous);
      return { ok: false, message };
    };
    // Every open module of the package is projected afresh from the edited
    // sources: values, and any structure an expression edit changed.
    const models: ViewModel[] = [];
    for (const module of open) {
      const shown = this.models.get(module);
      if (!shown || shown.module.package !== model.module.package) continue;
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
    this.projects.clear();
    this.models.clear();
    for (const raw of Object.values(fixtures)) {
      const model = parseViewModel(raw);
      this.models.set(model.module.id, model);
    }
    this.edited = false;
  }

  onModels(listener: (models: ViewModel[]) => void): void {
    this.modelListeners.push(listener);
  }

  onOpen(listener: (module: string) => void): void {
    this.openListeners.push(listener);
  }

  onNote(): void {
    // The dev shell has nothing to say on its own.
  }
}
