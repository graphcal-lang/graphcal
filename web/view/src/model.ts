import { z } from "zod";

// ViewModel: the presentation projection of one Graphcal module (one file).
// Structure (nodes, axes, dependencies) is small and always present; values are
// attached per node and may be absent for folded or unevaluated nodes.

export type Value =
  | { kind: "quantity"; display: string; si_value: number; unit: string | null }
  | { kind: "int"; display: string }
  | { kind: "bool"; display: string; value: boolean }
  | { kind: "indexed"; display: string; index: string; entries: { key: string; value: Value }[] };

const valueSchema: z.ZodType<Value> = z.lazy(() =>
  z.discriminatedUnion("kind", [
    z.object({
      kind: z.literal("quantity"),
      display: z.string(),
      si_value: z.number(),
      unit: z.string().nullable(),
    }),
    z.object({ kind: z.literal("int"), display: z.string() }),
    z.object({ kind: z.literal("bool"), display: z.string(), value: z.boolean() }),
    z.object({
      kind: z.literal("indexed"),
      display: z.string(),
      index: z.string(),
      entries: z.array(z.object({ key: z.string(), value: valueSchema })),
    }),
  ]),
);

export const nodeKindSchema = z.enum(["param", "const", "node", "assert", "external"]);
export type NodeKind = z.infer<typeof nodeKindSchema>;

export const viewNodeSchema = z.object({
  id: z.string(),
  name: z.string(),
  module: z.string(),
  kind: nodeKindSchema,
  axes: z.array(z.string()),
  dimension: z.string().nullable(),
  unit: z.string().nullable(),
  pub: z.boolean(),
  deps: z.array(z.string()),
  source: z.object({ file: z.string().nullable(), line: z.int().nonnegative() }).nullable(),
  value: valueSchema.nullable(),
});
export type ViewNode = z.infer<typeof viewNodeSchema>;

export const viewModelSchema = z.object({
  version: z.literal(0),
  module: z.object({ id: z.string(), file: z.string(), package: z.string() }),
  indexes: z.array(z.object({ name: z.string(), module: z.string(), labels: z.array(z.string()) })),
  nodes: z.array(viewNodeSchema),
});
export type ViewModel = z.infer<typeof viewModelSchema>;

export function parseViewModel(raw: unknown): ViewModel {
  return viewModelSchema.parse(raw);
}
