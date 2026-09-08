import { z } from "zod";

// This is a presentation projection of the Rust transport. Unused numeric/body
// fields are deliberately stripped; rendered fields are validated, never cast.
const position = z.object({ line: z.int().nonnegative(), character: z.int().nonnegative() });
export const rangeSchema = z.object({ start: position, end: position });
export type SourceRange = z.infer<typeof rangeSchema>;
export const diagnosticSchema = z.object({
  file: z.string(),
  severity: z.literal("error"),
  code: z.string().nullish(),
  message: z.string(),
  help: z.string().nullish(),
  labels: z.array(z.object({ message: z.string().nullish(), range: rangeSchema })),
});
export type Diagnostic = z.infer<typeof diagnosticSchema>;
export type Value =
  | { kind: "quantity" | "complex" | "bool" | "int" | "label" | "datetime"; display: string }
  | { kind: "struct"; display: string; fields: { name: string; value: Value }[] }
  | { kind: "indexed"; display: string; entries: { display_key: string; value: Value }[] };
export interface GridTable {
  columns: string[];
  rows: [string, string[]][];
}
export type ValueBody =
  | { kind: "scalar"; body: string }
  | { kind: "entries"; body: [string, string][] }
  | { kind: "grid"; body: GridTable }
  | { kind: "slices"; body: [string, GridTable][] };
const valueSchema: z.ZodType<Value> = z.lazy(() =>
  z.discriminatedUnion("kind", [
    z.object({
      kind: z.enum(["quantity", "complex", "bool", "int", "label", "datetime"]),
      display: z.string(),
    }),
    z.object({
      kind: z.literal("struct"),
      display: z.string(),
      fields: z.array(z.object({ name: z.string(), value: valueSchema })),
    }),
    z.object({
      kind: z.literal("indexed"),
      display: z.string(),
      entries: z.array(z.object({ display_key: z.string(), value: valueSchema })),
    }),
  ]),
);
const gridTableSchema = z.object({
  columns: z.array(z.string()),
  rows: z.array(z.tuple([z.string(), z.array(z.string())])),
});
const valueBodySchema: z.ZodType<ValueBody> = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("scalar"), body: z.string() }),
  z.object({ kind: z.literal("entries"), body: z.array(z.tuple([z.string(), z.string()])) }),
  z.object({ kind: z.literal("grid"), body: gridTableSchema }),
  z.object({ kind: z.literal("slices"), body: z.array(z.tuple([z.string(), gridTableSchema])) }),
]);
const nodeError = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("evaluation_failed"), message: z.string() }),
  z.object({ kind: z.literal("dependency_failed"), failed_dependencies: z.array(z.string()) }),
]);
export const outcomeSchema = z.discriminatedUnion("status", [
  z.object({ status: z.literal("rejected"), error: z.object({ message: z.string() }) }),
  z.object({ status: z.literal("compile_error"), diagnostics: z.array(diagnosticSchema) }),
  z.object({
    status: z.literal("evaluated"),
    evaluation: z.object({
      compiler_version: z.string(),
      has_errors: z.boolean(),
      values: z.array(
        z.object({
          name: z.string(),
          declaration_kind: z.enum(["const", "param", "node"]),
          outcome: z.discriminatedUnion("status", [
            z.object({ status: z.literal("value"), value: valueSchema, body: valueBodySchema }),
            z.object({ status: z.literal("error"), error: nodeError }),
          ]),
        }),
      ),
      assertions: z.array(
        z.object({
          name: z.string(),
          affected_declarations: z.array(z.string()),
          outcome: z.discriminatedUnion("status", [
            z.object({ status: z.literal("pass") }),
            z.object({ status: z.enum(["fail", "error"]), message: z.string() }),
          ]),
        }),
      ),
      notices: z.array(
        z.discriminatedUnion("kind", [
          z.object({ kind: z.literal("plot_error"), name: z.string(), message: z.string() }),
          z.object({ kind: z.enum(["presentation_error", "internal_error"]), message: z.string() }),
        ]),
      ),
      figures: z.array(z.object({ name: z.string(), spec: z.record(z.string(), z.unknown()) })),
    }),
  }),
]);
export type Outcome = z.infer<typeof outcomeSchema>;
export const workerReplySchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("ready") }),
  z.object({ kind: z.literal("result"), id: z.int(), outcome: outcomeSchema }),
  z.object({ kind: z.literal("error"), message: z.string() }),
]);
export type WorkerReply = z.infer<typeof workerReplySchema>;
