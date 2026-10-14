// The models a host has projected, and which of them still describe the
// sources it holds. An edit re-projects only the modules on screen; every other
// model then describes sources that are gone. Such a stale model is never handed
// out as current: its id and file only locate the module, to project it again.
import type { ViewModel } from "./model.ts";

export class ModelCache {
  private readonly models = new Map<string, ViewModel>();
  private readonly stale = new Set<string>();

  /** Keep a model projected from the current sources. */
  set(model: ViewModel): void {
    this.models.set(model.module.id, model);
    this.stale.delete(model.module.id);
  }

  /** The sources changed: `fresh` are projected from them, every other model is stale. */
  edited(fresh: readonly ViewModel[]): void {
    for (const id of this.models.keys()) this.stale.add(id);
    for (const model of fresh) this.set(model);
  }

  /** Start over with `models`, all current: a new project read, or a reset to the sources as read. */
  replace(models: Iterable<ViewModel>): void {
    this.models.clear();
    this.stale.clear();
    for (const model of models) this.set(model);
  }

  /** The model of `module` as of the current sources; null when there is none or it is stale. */
  current(module: string): ViewModel | null {
    return this.stale.has(module) ? null : (this.models.get(module) ?? null);
  }

  /** Every model as of the current sources. */
  currentModels(): ViewModel[] {
    return [...this.models.values()].filter((model) => !this.stale.has(model.module.id));
  }

  /** The last model of `module`, current or stale: where it is, not what it holds. */
  last(module: string): ViewModel | null {
    return this.models.get(module) ?? null;
  }

  /** Whether `module` has a model, but one from sources since edited. */
  isStale(module: string): boolean {
    return this.stale.has(module);
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
