// The refs of the repository being read, listed once per repository and
// token: the ref menu opens again without asking GitHub, and moving to
// another repository (or signing in) lists that one's.
import type { Refs } from "./github.ts";

export interface RefsKey {
  owner: string;
  repo: string;
  /** The token the list was read with; another may see more (private pull requests, say). */
  token: string | null;
}

function sameKey(a: RefsKey, b: RefsKey): boolean {
  return a.owner === b.owner && a.repo === b.repo && a.token === b.token;
}

export class RefsCache {
  private last: { key: RefsKey; refs: Promise<Refs> } | null = null;

  /** The refs for `key`, read with `load` unless the last read was for the same key. A failed read is not kept. */
  get(key: RefsKey, load: () => Promise<Refs>): Promise<Refs> {
    if (this.last !== null && sameKey(this.last.key, key)) return this.last.refs;
    const refs = load();
    const entry = { key, refs };
    this.last = entry;
    refs.catch(() => {
      if (this.last === entry) this.last = null;
    });
    return refs;
  }
}
