// Builds a ViewModel fixture from a hand-written structure file and the JSON
// output of `graphcal eval --format json --output-view all` (run on the sample
// project under fixtures/<name>_project). This stands in for
// the ViewModel export the compiler will provide; delete it once that exists.
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const name = process.argv[2] ?? "power_max";
const structure = JSON.parse(readFileSync(join(here, `${name}.structure.json`), "utf8"));
const evaluated = JSON.parse(readFileSync(join(here, `${name}.eval.json`), "utf8"));

const labelsOf = Object.fromEntries(structure.indexes.map((i) => [i.name, i.labels]));

function formatNumber(x) {
  if (!Number.isFinite(x)) return String(x);
  const rounded = Number(x.toPrecision(4));
  return rounded.toLocaleString("en-US", { maximumFractionDigits: 6, useGrouping: false });
}

function convert(raw, node) {
  if (typeof raw === "number") return { kind: "int", display: String(raw) };
  if (typeof raw === "boolean")
    return { kind: "bool", display: raw ? "true" : "false", value: raw };
  if (raw && typeof raw === "object") {
    if ("status" in raw) {
      const pass = raw.status === "pass";
      return { kind: "bool", display: pass ? "PASS" : "FAIL", value: pass };
    }
    if ("si_value" in raw) {
      const unit = node.unit;
      const magnitude = formatNumber(raw.si_value);
      const display = unit ? `${magnitude} ${unit}` : magnitude;
      return { kind: "quantity", display, magnitude, si_value: raw.si_value, unit };
    }
    if ("entries" in raw) {
      const order = labelsOf[raw.index] ?? Object.keys(raw.entries);
      return {
        kind: "indexed",
        display: `[${raw.index}]`,
        index: raw.index,
        entries: order.map((key) => ({ key, value: convert(raw.entries[key], node) })),
      };
    }
  }
  throw new Error(`unrecognized value for ${node.name}: ${JSON.stringify(raw)}`);
}

function lookup(name) {
  for (const section of ["param", "const", "node", "assert"]) {
    const table = evaluated[section];
    if (table && name in table) return table[name];
  }
  return undefined;
}

const nodes = structure.nodes.map((node) => {
  const raw = lookup(node.name);
  return {
    id: node.name,
    name: node.name,
    module: node.module,
    kind: node.kind,
    axes: node.axes,
    dimension: node.dimension,
    unit: node.unit,
    pub: node.pub,
    deps: node.deps,
    expr: node.expr ?? null,
    reduce: node.reduce ?? null,
    source: {
      file: node.module === structure.module.id ? structure.module.file : null,
      line: node.line,
    },
    value: raw === undefined ? null : convert(raw, node),
  };
});

const model = { version: 0, module: structure.module, indexes: structure.indexes, nodes };
writeFileSync(join(here, `${name}.view.json`), `${JSON.stringify(model, null, 2)}\n`);
console.log(`wrote fixtures/${name}.view.json (${nodes.length} nodes)`);
