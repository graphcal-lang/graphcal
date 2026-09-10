import { z } from "zod";

const utf8 = (maximum: number) =>
  z
    .string()
    .refine((value) => value.isWellFormed() && new TextEncoder().encode(value).length <= maximum);

/** Bound before crossing either the iframe or Wasm boundary. Rust checks syntax and units. */
export const bindingsSchema = z
  .array(
    z.strictObject({
      name: utf8(1024).refine((name) => name.length > 0),
      expr: utf8(4096),
    }),
  )
  .max(256)
  .refine(
    (bindings) => new Set(bindings.map((binding) => binding.name)).size === bindings.length,
    "Specify each parameter only once.",
  );
export type Binding = z.infer<typeof bindingsSchema>[number];

export function sameBindings(left: Binding[], right: Binding[]) {
  return (
    left.length === right.length &&
    left.every((binding) =>
      right.some((other) => binding.name === other.name && binding.expr === other.expr),
    )
  );
}
