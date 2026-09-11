import { z } from "zod";

const utf8 = (maximum: number) =>
  z
    .string()
    .refine((value) => value.isWellFormed() && new TextEncoder().encode(value).length <= maximum);

export type StructuredValue =
  | { kind: "literal"; expr: string }
  | {
      kind: "algebraic";
      definition: number;
      constructor: number;
      fields: StructuredValue[];
    }
  | { kind: "indexed"; entries: StructuredValue[] };

const structuredValueSchema: z.ZodType<StructuredValue> = z.lazy(() =>
  z.discriminatedUnion("kind", [
    z.strictObject({ kind: z.literal("literal"), expr: utf8(4096) }),
    z.strictObject({
      kind: z.literal("algebraic"),
      definition: z.number().int().nonnegative(),
      constructor: z.number().int().nonnegative(),
      fields: z.array(structuredValueSchema).max(4096),
    }),
    z.strictObject({
      kind: z.literal("indexed"),
      entries: z.array(structuredValueSchema).max(4096),
    }),
  ]),
);

const bindingSchema = z.union([
  z.strictObject({
    name: utf8(1024).refine((name) => name.length > 0),
    expr: utf8(4096),
  }),
  z.strictObject({
    name: utf8(1024).refine((name) => name.length > 0),
    value: structuredValueSchema,
  }),
]);

const structuredEnvelopeSchema = z
  .array(z.unknown())
  .max(256)
  .superRefine((bindings, context) => {
    for (const binding of bindings) {
      if (!binding || typeof binding !== "object" || !("value" in binding)) continue;
      const stack: { value: unknown; depth: number }[] = [
        { value: (binding as { value: unknown }).value, depth: 0 },
      ];
      let nodes = 0;
      while (stack.length > 0) {
        const current = stack.pop();
        if (!current) break;
        nodes += 1;
        if (current.depth > 32) {
          context.addIssue({ code: "custom", message: "Structured binding is too deeply nested." });
          break;
        }
        if (nodes > 4096) {
          context.addIssue({ code: "custom", message: "Structured binding is too large." });
          break;
        }
        if (!current.value || typeof current.value !== "object") continue;
        const record = current.value as Record<string, unknown>;
        if (
          record.kind === "literal" &&
          typeof record.expr === "string" &&
          new TextEncoder().encode(record.expr).length > 4096
        ) {
          context.addIssue({ code: "custom", message: "Structured binding leaf is too large." });
          break;
        }
        const children = record.kind === "algebraic" ? record.fields : record.entries;
        if (!Array.isArray(children)) continue;
        for (const value of children) stack.push({ value, depth: current.depth + 1 });
      }
    }
  });

/** Bound before crossing either the iframe or Wasm boundary. Rust checks schema, syntax and units. */
export const bindingsSchema = structuredEnvelopeSchema.pipe(
  z.array(bindingSchema).superRefine((bindings, context) => {
    if (new Set(bindings.map((binding) => binding.name)).size !== bindings.length) {
      context.addIssue({ code: "custom", message: "Specify each parameter only once." });
    }
  }),
);
export type Binding = z.infer<typeof bindingsSchema>[number];

export function sameBindings(left: Binding[], right: Binding[]) {
  const byName = (bindings: Binding[]) =>
    [...bindings].sort((a, b) => a.name.localeCompare(b.name));
  return JSON.stringify(byName(left)) === JSON.stringify(byName(right));
}
