// What a sheet shows, and what a host is asked for: a module evaluated alone,
// or one include instance of a root module, evaluated as the root evaluates
// it (with the include's arguments). The two are different targets even
// when the include binds nothing, so models, panes and requests are keyed by
// target, never by module alone. Instance ids are opaque: they are compared
// and sent back, never taken apart.
import type { ViewModel, ViewNode } from "./model.ts";

export type ViewTarget =
  | { kind: "module"; module: string }
  /** The instance `instance` (an opaque id) in the evaluation of the module `root`. */
  | { kind: "instance"; root: string; instance: string };

export function moduleTarget(module: string): ViewTarget {
  return { kind: "module", module };
}

export function sameTarget(a: ViewTarget, b: ViewTarget): boolean {
  switch (a.kind) {
    case "module":
      return b.kind === "module" && a.module === b.module;
    case "instance":
      return b.kind === "instance" && a.root === b.root && a.instance === b.instance;
  }
}

/** The target a model was projected as. */
export function targetOf(model: ViewModel): ViewTarget {
  return model.target.kind === "module"
    ? moduleTarget(model.module.id)
    : { kind: "instance", root: model.target.root.id, instance: model.target.instance.id };
}

/** The module whose evaluation a target is part of: the module itself, or the instance's root. */
export function rootOf(target: ViewTarget): string {
  return target.kind === "module" ? target.module : target.root;
}

/**
 * The target a sheet showing `reader` reads the external `node` from: an
 * imported module alone, or the include instance in the same root's
 * evaluation. Null for what no sheet of its own shows: an inline call's
 * output (evaluated in place) and, in an instance's view, the includer's
 * declaration (the includer's own sheet).
 */
export function readTarget(reader: ViewTarget, node: ViewNode): ViewTarget | null {
  switch (node.origin.kind) {
    case "imported":
      return moduleTarget(node.module);
    case "included":
      return { kind: "instance", root: rootOf(reader), instance: node.origin.instance.id };
    case "local":
    case "called":
    case "includer":
      return null;
  }
}

/** A map keyed by target. */
export class TargetMap<V> {
  private readonly modules = new Map<string, V>();
  /** By root module, then by instance id. */
  private readonly instances = new Map<string, Map<string, V>>();

  get(target: ViewTarget): V | undefined {
    return target.kind === "module"
      ? this.modules.get(target.module)
      : this.instances.get(target.root)?.get(target.instance);
  }

  set(target: ViewTarget, value: V): void {
    if (target.kind === "module") {
      this.modules.set(target.module, value);
      return;
    }
    const byId = this.instances.get(target.root) ?? new Map<string, V>();
    byId.set(target.instance, value);
    this.instances.set(target.root, byId);
  }

  clear(): void {
    this.modules.clear();
    this.instances.clear();
  }

  /** Every target with a value: the modules first, then the instances by root. */
  entries(): [ViewTarget, V][] {
    const out: [ViewTarget, V][] = [...this.modules].map(([module, value]) => [
      moduleTarget(module),
      value,
    ]);
    for (const [root, byId] of this.instances) {
      for (const [instance, value] of byId) {
        out.push([{ kind: "instance", root, instance }, value]);
      }
    }
    return out;
  }

  /** The modules with a value, as targets of their own. */
  moduleIds(): string[] {
    return [...this.modules.keys()];
  }
}
