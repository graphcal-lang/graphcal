// The models a host has projected, by target, each with the revision of the
// sources it was projected from. Only a model of the current revision is
// handed out as current: an edit re-projects the targets on screen, and every
// other model then describes sources that are gone. A stale model's target
// and files only locate it, to project it again; a projection that finishes
// after the sources moved on is recognised by its revision and not kept.
import type { ViewModel } from "./model.ts";
import { TargetMap, targetOf, type ViewTarget } from "./target.ts";

export class ModelCache {
  private readonly models = new TargetMap<ViewModel>();
  private readonly revisions = new WeakMap<ViewModel, number>();
  /** The revision of the host's sources. */
  private revision = 0;

  /** Keep a model projected at `revision`; false (and not kept) when the sources moved on since. */
  set(model: ViewModel, revision: number): boolean {
    if (revision < this.revision) return false;
    this.models.set(targetOf(model), model);
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

  /** The model of `target` as of the current sources; null when there is none or it is stale. */
  current(target: ViewTarget): ViewModel | null {
    const model = this.models.get(target);
    return model !== undefined && !this.isStale(target) ? model : null;
  }

  /** Every model as of the current sources. */
  currentModels(): ViewModel[] {
    return this.models
      .entries()
      .map(([target]) => this.current(target))
      .filter((model): model is ViewModel => model !== null);
  }

  /** The last model of `target`, current or stale: where it is, not what it holds. */
  last(target: ViewTarget): ViewModel | null {
    return this.models.get(target) ?? null;
  }

  /** Whether `target` has a model, but one from sources since edited. */
  isStale(target: ViewTarget): boolean {
    const model = this.models.get(target);
    return model !== undefined && (this.revisions.get(model) ?? -1) < this.revision;
  }

  /** Every module with a model of its own (evaluated alone), current or stale, sorted. */
  moduleIds(): string[] {
    return this.models.moduleIds().sort();
  }

  /** The module whose own model presents the file `file`, current or stale. */
  moduleOfFile(file: string): string | null {
    for (const [target, model] of this.models.entries()) {
      if (target.kind === "module" && model.module.file === file) return target.module;
    }
    return null;
  }
}
