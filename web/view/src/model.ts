import { z } from "zod";

// ViewModel: the presentation projection of one Graphcal module (one file).
// Structure (nodes, axes, dependencies) is small and always present; values are
// attached per node and may be absent for folded or unevaluated nodes.
//
// Ids (`id`, `deps`, `axes`, entry `key`s) are opaque: the page compares and
// looks them up, and never takes them apart. What the page shows travels in
// fields of its own (`label`, `name`, `display`).

export type Value =
  | { kind: "quantity"; display: string; magnitude: string; si_value: number; unit: string | null }
  | { kind: "int"; display: string }
  | { kind: "bool"; display: string; value: boolean }
  | { kind: "label"; display: string; index: string; variant: string }
  /** Entries follow the node's axes, outermost first; `key` is an entry key of that axis. */
  | { kind: "indexed"; display: string; index: string; entries: { key: string; value: Value }[] }
  /** Anything the view has no richer shape for (structs, complex, datetimes). */
  | { kind: "text"; display: string };

const valueSchema: z.ZodType<Value> = z.lazy(() =>
  z.discriminatedUnion("kind", [
    z.object({
      kind: z.literal("quantity"),
      display: z.string(),
      magnitude: z.string(),
      si_value: z.number(),
      unit: z.string().nullable(),
    }),
    z.object({ kind: z.literal("int"), display: z.string() }),
    z.object({ kind: z.literal("bool"), display: z.string(), value: z.boolean() }),
    z.object({
      kind: z.literal("label"),
      display: z.string(),
      index: z.string(),
      variant: z.string(),
    }),
    z.object({
      kind: z.literal("indexed"),
      display: z.string(),
      index: z.string(),
      entries: z.array(z.object({ key: z.string(), value: valueSchema })),
    }),
    z.object({ kind: z.literal("text"), display: z.string() }),
  ]),
);

export const nodeKindSchema = z.enum(["param", "const", "node", "assert"]);
export type NodeKind = z.infer<typeof nodeKindSchema>;

const instanceSchema = z.object({ name: z.string(), bound: z.boolean() });
export type Instance = z.infer<typeof instanceSchema>;

/**
 * Where a node's declaration comes from. An include member names the instance
 * it is read through (`eps` in `include power_project.eps() …`); a bound
 * instance (the include passes arguments) has values of its own, which no
 * view of its module evaluated alone shows.
 */
export const originSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("local") }),
  z.object({ kind: z.literal("imported") }),
  z.object({ kind: z.literal("included"), instance: instanceSchema }),
]);
export type Origin = z.infer<typeof originSchema>;

export const reduceOpSchema = z.enum(["sum", "maximum", "minimum", "count", "rss", "mean"]);
export type ReduceOp = z.infer<typeof reduceOpSchema>;

const positionSchema = z.object({ line: z.int().nonnegative(), character: z.int().nonnegative() });
export type Position = z.infer<typeof positionSchema>;
export type Range = { start: Position; end: Position };

export const viewNodeSchema = z.object({
  /** Opaque, unique within the model and stable across evaluations. */
  id: z.string(),
  /** The node as the module spells it: `margin`, `num`, `e::p_budget`. */
  label: z.string(),
  name: z.string(),
  module: z.string(),
  kind: nodeKindSchema,
  origin: originSchema,
  /** Index ids (`indexes[].id`), outermost first. */
  axes: z.array(z.string()),
  dimension: z.string().nullable(),
  unit: z.string().nullable(),
  pub: z.boolean(),
  /** Ids of the nodes this one reads. */
  deps: z.array(z.string()),
  /** Source expression text, for the formula bar. */
  expr: z.string().nullable(),
  /** The aggregation that removed an axis of a dependency, if any. */
  reduce: z.object({ op: reduceOpSchema, axis: z.string() }).nullable(),
  /** The declaration's `///` documentation, lines joined with newlines. */
  doc: z.string().nullable().optional(),
  source: z
    .object({
      file: z.string().nullable(),
      line: z.int().nonnegative(),
      /** Editor range of the defining expression (0-based lines, UTF-16 columns). */
      expr_range: z.object({ start: positionSchema, end: positionSchema }).nullable().optional(),
    })
    .nullable(),
  value: valueSchema.nullable(),
});
export type ViewNode = z.infer<typeof viewNodeSchema>;

export const indexEntrySchema = z.object({
  /** Opaque; the entries of an indexed value refer to it. */
  key: z.string(),
  /** What a reader sees: a variant, a coordinate (`0.5 s`), a position (`#2`). */
  display: z.string(),
});
export type IndexEntry = z.infer<typeof indexEntrySchema>;

export const viewIndexSchema = z.object({
  id: z.string(),
  name: z.string(),
  /** The declaring module; null for a structural `Fin(N)` axis. */
  module: z.string().nullable(),
  entries: z.array(indexEntrySchema),
});
export type ViewIndex = z.infer<typeof viewIndexSchema>;

export const viewModelSchema = z.object({
  version: z.literal(0),
  module: z.object({ id: z.string(), file: z.string(), package: z.string() }),
  indexes: z.array(viewIndexSchema),
  nodes: z.array(viewNodeSchema),
});
export type ViewModel = z.infer<typeof viewModelSchema>;

export function parseViewModel(raw: unknown): ViewModel {
  return viewModelSchema.parse(raw);
}

/** Whether a node is a declaration of another module that this one reads. */
export function isExternal(node: ViewNode): boolean {
  return node.origin.kind !== "local";
}

/** The include instance a node is read through, if any. */
export function instanceOf(node: ViewNode): Instance | null {
  return node.origin.kind === "included" ? node.origin.instance : null;
}
