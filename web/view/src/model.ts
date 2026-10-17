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

const moduleSchema = z.object({ id: z.string(), file: z.string(), package: z.string() });
export type ViewModule = z.infer<typeof moduleSchema>;

export const nodeKindSchema = z.enum(["param", "const", "node", "assert"]);
export type NodeKind = z.infer<typeof nodeKindSchema>;

/**
 * An include instance: `id` (opaque) asks for the instance's own view, `name`
 * is what a reader sees (`eps` in `include power_project.eps() …`). A bound
 * instance (the include passes arguments) has values of its own.
 */
const instanceSchema = z.object({ id: z.string(), name: z.string(), bound: z.boolean() });
export type Instance = z.infer<typeof instanceSchema>;

/** An inline DAG call (`scale(…)`): evaluated in place, it has no view of its own. */
const callSchema = z.object({ name: z.string(), bound: z.boolean() });
export type Call = z.infer<typeof callSchema>;

/**
 * Where a node's declaration comes from. An include member names the instance
 * it is read through, and the output of an inline call names the call
 * (`scale(…)` for `@scale(…)::out`); a call is evaluated in place, so its
 * output has no value. In an instance's view, `includer` is a declaration of
 * the module that includes the instance, read by the include's arguments.
 */
export const originSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("local") }),
  z.object({ kind: z.literal("imported") }),
  z.object({ kind: z.literal("included"), instance: instanceSchema }),
  z.object({ kind: z.literal("called"), instance: callSchema }),
  z.object({ kind: z.literal("includer") }),
]);
export type Origin = z.infer<typeof originSchema>;

const positionSchema = z.object({ line: z.int().nonnegative(), character: z.int().nonnegative() });
export type Position = z.infer<typeof positionSchema>;
const rangeSchema = z.object({ start: positionSchema, end: positionSchema });
export type Range = z.infer<typeof rangeSchema>;

export const elementSchema = z.enum([
  "bool",
  "int",
  "quantity",
  "complex",
  "datetime",
  "key",
  "struct",
  "other",
]);
export type Element = z.infer<typeof elementSchema>;

/** One written cell of a table definition; with the node id, `keys` identify the slot. */
const cellSchema = z.object({ keys: z.array(z.string()), range: rangeSchema });
export type Cell = z.infer<typeof cellSchema>;

export const viewNodeSchema = z.object({
  /** Opaque, unique within the model and stable across evaluations. */
  id: z.string(),
  /** The node as the module spells it: `margin`, `num`, `e::p_budget`. */
  label: z.string(),
  name: z.string(),
  module: z.string(),
  kind: nodeKindSchema,
  origin: originSchema,
  /** What one value (one cell) is; a typed literal for a cell depends on it. */
  element: elementSchema,
  /** Index ids (`indexes[].id`), outermost first. */
  axes: z.array(z.string()),
  dimension: z.string().nullable(),
  unit: z.string().nullable(),
  pub: z.boolean(),
  /** Ids of the nodes this one reads. */
  deps: z.array(z.string()),
  /** Source expression text, for the formula bar. */
  expr: z.string().nullable(),
  /** The declaration's `///` documentation, lines joined with newlines. */
  doc: z.string().nullable().optional(),
  source: z
    .object({
      file: z.string().nullable(),
      line: z.int().nonnegative(),
      /**
       * Editor range of the defining expression (0-based lines, UTF-16 columns),
       * parentheses included; null when no single declaration owns the text
       * (the columns of a multi-declaration table share one).
       */
      expr_range: rangeSchema.nullable(),
      /** The written cells of a table definition: entry keys per axis and the cell's range. */
      cells: z.array(cellSchema),
    })
    .nullable(),
  value: valueSchema.nullable(),
  /** Why parts of the value are shown in SI instead of their display unit. */
  presentation: z.array(z.string()),
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
  version: z.literal(1),
  /** The module whose declarations the view presents (for an instance, its template). */
  module: moduleSchema,
  /**
   * What the view presents the module as: evaluated alone, or as one include
   * instance of `root` evaluated as the root, with the include's arguments.
   */
  target: z.discriminatedUnion("kind", [
    z.object({ kind: z.literal("module") }),
    z.object({ kind: z.literal("instance"), root: moduleSchema, instance: instanceSchema }),
  ]),
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

/** The include or call instance a node is read through, if any. */
export function instanceOf(node: ViewNode): Instance | Call | null {
  return node.origin.kind === "included" || node.origin.kind === "called"
    ? node.origin.instance
    : null;
}
