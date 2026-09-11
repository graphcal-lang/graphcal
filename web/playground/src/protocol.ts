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
  | {
      kind: "quantity";
      display: string;
      literal: string;
      value: number;
      si_value: number;
      unit?: string | null;
    }
  | { kind: "bool"; display: string; literal: string; value: boolean }
  | { kind: "int"; display: string; literal: string; decimal: string }
  | { kind: "label"; display: string; literal: string; index: string; variant: string }
  | { kind: "complex" | "datetime"; display: string; literal: string }
  | { kind: "struct"; display: string; type_name: string; fields: { name: string; value: Value }[] }
  | {
      kind: "indexed";
      display: string;
      index: string;
      entries: { display_key: string; value: Value }[];
    };
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
      kind: z.literal("quantity"),
      display: z.string(),
      literal: z.string(),
      value: z.number(),
      unit: z.string().nullish(),
      si_value: z.number(),
    }),
    z.object({
      kind: z.literal("bool"),
      display: z.string(),
      literal: z.string(),
      value: z.boolean(),
    }),
    z.object({
      kind: z.literal("int"),
      display: z.string(),
      literal: z.string(),
      decimal: z.string(),
    }),
    z.object({
      kind: z.literal("label"),
      display: z.string(),
      literal: z.string(),
      index: z.string(),
      variant: z.string(),
    }),
    z.object({ kind: z.enum(["complex", "datetime"]), display: z.string(), literal: z.string() }),
    z.object({
      kind: z.literal("struct"),
      display: z.string(),
      type_name: z.string(),
      fields: z.array(z.object({ name: z.string(), value: valueSchema })),
    }),
    z.object({
      kind: z.literal("indexed"),
      display: z.string(),
      index: z.string(),
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
export const reportOutcomeSchema = z.discriminatedUnion("status", [
  outcomeSchema.options[0],
  outcomeSchema.options[1],
  outcomeSchema.options[2].extend({ html: z.string() }),
  z.object({
    status: z.literal("binding_errors"),
    errors: z.array(
      z.object({
        name: z.string(),
        message: z.string(),
        path: z
          .array(
            z.discriminatedUnion("kind", [
              z.object({ kind: z.literal("field"), index: z.int().nonnegative() }),
              z.object({ kind: z.literal("entry"), index: z.int().nonnegative() }),
            ]),
          )
          .default([]),
      }),
    ),
  }),
  z.object({ status: z.literal("eval_error"), message: z.string() }),
]);
export type ReportOutcome = z.infer<typeof reportOutcomeSchema>;
const indexValueSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("named"), name: z.string(), variants: z.array(z.string()) }),
  z.object({ kind: z.literal("coordinate"), name: z.string(), labels: z.array(z.string()) }),
  z.object({ kind: z.literal("finite"), cardinality: z.int().nonnegative() }),
]);
const recursiveValueSchema: z.ZodType<unknown> = z.lazy(() =>
  z.discriminatedUnion("kind", [
    z.object({ kind: z.literal("quantity"), unit: z.string().nullish() }),
    z.object({ kind: z.literal("complex"), unit: z.string().nullish() }),
    z.object({ kind: z.literal("boolean") }),
    z.object({ kind: z.literal("integer") }),
    z.object({ kind: z.literal("datetime"), time_scale: z.string() }),
    z.object({ kind: z.literal("key"), axis: indexValueSchema }),
    z.object({ kind: z.literal("algebraic"), definition: z.int().nonnegative() }),
    z.object({ kind: z.literal("indexed"), axis: indexValueSchema, element: recursiveValueSchema }),
  ]),
);
export const portsSchema = z.array(
  z.object({
    name: z.string(),
    has_default: z.boolean(),
    control: z.discriminatedUnion("kind", [
      z.object({
        kind: z.literal("quantity"),
        unit: z.string().nullish(),
        lower_si: z.number().nullish(),
        upper_si: z.number().nullish(),
      }),
      z.object({
        kind: z.literal("integer"),
        lower: z.string().nullish(),
        upper: z.string().nullish(),
      }),
      z.object({ kind: z.literal("boolean") }),
      z.object({ kind: z.literal("select"), index: z.string(), variants: z.array(z.string()) }),
      z.object({ kind: z.literal("datetime"), time_scale: z.string() }),
      z.object({ kind: z.literal("expression") }),
    ]),
    schema: recursiveValueSchema,
    definitions: z.array(
      z.object({
        id: z.int().nonnegative(),
        constructors: z.array(
          z.object({
            id: z.int().nonnegative(),
            name: z.string(),
            fields: z.array(
              z.object({
                id: z.int().nonnegative(),
                name: z.string(),
                schema: recursiveValueSchema,
              }),
            ),
          }),
        ),
      }),
    ),
  }),
);
export type ParameterPort = z.infer<typeof portsSchema>[number];
export const workerReplySchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("ready") }),
  z.object({
    kind: z.literal("result"),
    id: z.int(),
    outcome: reportOutcomeSchema,
    ports: portsSchema.default([]),
  }),
  z.object({ kind: z.literal("error"), message: z.string() }),
]);
export type WorkerReply = z.infer<typeof workerReplySchema>;
