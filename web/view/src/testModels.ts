// Models for tests: a node with every field filled in, overridden per test.
import type { Value, ViewModel, ViewNode } from "./model.ts";

export function viewNode(id: string, extra: Partial<ViewNode> = {}): ViewNode {
  return {
    id,
    name: id,
    module: "p.m",
    kind: "param",
    axes: [],
    dimension: "Power",
    unit: "W",
    pub: false,
    deps: [],
    expr: null,
    label: id,
    origin: { kind: "local" },
    element: "quantity",
    presentation: [],
    source: null,
    value: null,
    ...extra,
  };
}

export function viewModel(nodes: ViewNode[], extra: Partial<ViewModel> = {}): ViewModel {
  return {
    version: 1,
    module: { id: "p.m", file: "src/m.gcl", package: "p" },
    target: { kind: "module" },
    indexes: [],
    nodes,
    ...extra,
  };
}

export function quantity(magnitude: string, unit: string | null): Value {
  return {
    kind: "quantity",
    display: unit === null ? magnitude : `${magnitude} ${unit}`,
    magnitude,
    si_value: Number(magnitude),
    unit,
  };
}
