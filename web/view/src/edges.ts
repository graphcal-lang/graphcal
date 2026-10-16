// A value per directed edge between two nodes, keyed by the pair of node ids
// itself: the ids are opaque, so the pair is never joined into one string.

export class EdgeMap<T> {
  private readonly byFrom = new Map<string, Map<string, T>>();

  set(from: string, to: string, value: T): void {
    const row = this.byFrom.get(from) ?? new Map<string, T>();
    row.set(to, value);
    this.byFrom.set(from, row);
  }

  get(from: string, to: string): T | undefined {
    return this.byFrom.get(from)?.get(to);
  }

  has(from: string, to: string): boolean {
    return this.byFrom.get(from)?.has(to) ?? false;
  }

  get size(): number {
    let n = 0;
    for (const row of this.byFrom.values()) n += row.size;
    return n;
  }
}
