// The models a host has projected, each with the revision of the sources it
// was projected from. Only a model of the current revision is handed out as
// current: an edit re-projects the modules on screen, and every other model
// then describes sources that are gone. A stale model's id and file only
// locate the module, to project it again; a projection that finishes after
// the sources moved on is recognised by its revision and not kept.
import type { ViewModel } from "./model.ts";

export class ModelCache {
  private readonly models = new Map<string, ViewModel>();
  private readonly revisions = new WeakMap<ViewModel, number>();
  /** The revision of the host's sources. */
  private revision = 0;

  /** Keep a model projected at `revision`; false (and not kept) when the sources moved on since. */
  set(model: ViewModel, revision: number): boolean {
    if (revision < this.revision) return false;
    this.models.set(model.module.id, model);
    this.revisions.set(model, revision);
    return true;
  }

  /** The sources are now at `revision`: `fresh` are projected from them, every other model is stale. */
  edited(fresh: readonly ViewModel[], revision: number): void {
    this.revision = revision;
    for (const model of fresh) this.set(model, revision);
  }

  /** Start over at `revision` with `models`, projected from it: a new project read, or a reset. */
  replace(models: Iterable<ViewModel>, revision: number): void {
    this.models.clear();
    this.revision = revision;
    for (const model of models) this.set(model, revision);
  }

  /** The revision a model was projected from, when this cache keeps it. */
  revisionOf(model: ViewModel): number | null {
    return this.revisions.get(model) ?? null;
  }

  /** The model of `module` as of the current sources; null when there is none or it is stale. */
  current(module: string): ViewModel | null {
    const model = this.models.get(module);
    return model !== undefined && !this.isStale(module) ? model : null;
  }

  /** Every model as of the current sources. */
  currentModels(): ViewModel[] {
    return [...this.models.keys()]
      .map((module) => this.current(module))
      .filter((model): model is ViewModel => model !== null);
  }

  /** The last model of `module`, current or stale: where it is, not what it holds. */
  last(module: string): ViewModel | null {
    return this.models.get(module) ?? null;
  }

  /** Whether `module` has a model, but one from sources since edited. */
  isStale(module: string): boolean {
    const model = this.models.get(module);
    return model !== undefined && (this.revisions.get(model) ?? -1) < this.revision;
  }

  /** Every module with a model, current or stale, sorted. */
  ids(): string[] {
    return [...this.models.keys()].sort();
  }

  /** The module whose file is `file`, current or stale. */
  moduleOfFile(file: string): string | null {
    for (const model of this.models.values()) {
      if (model.module.file === file) return model.module.id;
    }
    return null;
  }
}
