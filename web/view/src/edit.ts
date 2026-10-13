// Fold an engine evaluation back into the ViewModel: values, the axes a value
// turned out to have, and the dependencies an edited expression mentions.
import type { EngineValue, Evaluation } from "./engine.ts";
import type { Value, ViewModel, ViewNode } from "./model.ts";

function formatNumber(x: number): string {
  if (!Number.isFinite(x)) return String(x);
  const rounded = Number(x.toPrecision(4));
  return rounded.toLocaleString("en-US", { maximumFractionDigits: 6, useGrouping: false });
}

function toValue(raw: EngineValue, node: ViewNode): Value {
  switch (raw.kind) {
    case "quantity": {
      const unit = node.unit ?? raw.unit ?? null;
      const magnitude = formatNumber(raw.value);
      return {
        kind: "quantity",
        display: unit ? `${magnitude} ${unit}` : magnitude,
        magnitude,
        si_value: raw.si_value,
        unit,
      };
    }
    case "int":
      return { kind: "int", display: raw.decimal };
    case "bool":
      return { kind: "bool", display: raw.value ? "true" : "false", value: raw.value };
    case "label":
      return { kind: "label", display: raw.variant, index: raw.index, variant: raw.variant };
    case "indexed":
      return {
        kind: "indexed",
        display: `[${raw.index}]`,
        index: raw.index,
        entries: raw.entries.map((e) => ({ key: e.display_key, value: toValue(e.value, node) })),
      };
    default:
      return { kind: "text", display: raw.display };
  }
}

function axesOf(value: Value): string[] {
  const axes: string[] = [];
  let current: Value | null = value;
  while (current && current.kind === "indexed") {
    axes.push(current.index);
    current = current.entries[0]?.value ?? null;
  }
  return axes;
}

function bareName(name: string): string {
  return name.split("::").at(-1) ?? name;
}

/** Write an evaluation's values into the model in place; unknown names are ignored. */
export function applyEvaluation(model: ViewModel, evaluation: Evaluation): void {
  const byName = new Map(model.nodes.map((n) => [n.id, n]));
  for (const decl of evaluation.values) {
    const node = byName.get(bareName(decl.name));
    if (!node) continue;
    if (decl.outcome.status !== "value") {
      node.value = null;
      continue;
    }
    node.value = toValue(decl.outcome.value, node);
    const axes = axesOf(node.value);
    if (axes.join("\0") !== node.axes.join("\0")) node.axes = axes;
  }
  for (const assertion of evaluation.assertions) {
    const node = byName.get(bareName(assertion.name));
    if (!node) continue;
    const pass = assertion.outcome.status === "pass";
    node.value = { kind: "bool", display: pass ? "PASS" : "FAIL", value: pass };
  }
}

/** The node ids an expression refers to with `@name` or `@module::name`. */
export function depsOf(expr: string, model: ViewModel): string[] {
  const ids = new Set(model.nodes.map((n) => n.id));
  const out: string[] = [];
  for (const m of expr.matchAll(/@(?:[a-zA-Z_][\w.]*\s*::\s*)?([a-zA-Z_]\w*)/g)) {
    const id = m[1] ?? "";
    if (ids.has(id) && !out.includes(id)) out.push(id);
  }
  return out;
}
