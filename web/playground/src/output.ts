import { element } from "./dom";
import { renderFigure } from "./figures";
import type { GridTable, Outcome, Value, ValueBody, SourceRange } from "./protocol";

function paged<T>(
  parent: HTMLElement,
  values: readonly T[],
  render: (value: T) => HTMLElement,
  controls: HTMLElement = parent,
) {
  let offset = 0;
  const more = element("button", "Show more");
  more.type = "button";
  function next() {
    more.remove();
    values.slice(offset, offset + 100).forEach((value) => parent.append(render(value)));
    offset += 100;
    if (offset < values.length) {
      more.textContent = `Show more (${values.length - offset} remaining)`;
      controls.append(more);
    }
  }
  more.addEventListener("click", next);
  next();
}
function listDetails(
  display: string,
  entries: readonly { name: string; value: Value }[],
): HTMLDetailsElement {
  const details = element("details");
  details.append(element("summary", display));
  details.addEventListener("toggle", () => {
    if (!details.open || details.childElementCount > 1) return;
    paged(details, entries, (entry) => {
      const row = element("div", undefined, "value-row");
      row.append(element("code", entry.name), valueNode(entry.value));
      return row;
    });
  });
  return details;
}

function tableNode(grid: GridTable, sliceLabel?: string): HTMLElement {
  const section = element("section", undefined, "table-slice");
  const scroller = element("div", undefined, "value-table-scroll");
  const table = element("table", undefined, "value-table");
  const caption = element("caption", sliceLabel ? `[${sliceLabel}]` : "Indexed value");
  if (sliceLabel) caption.className = "table-selector";
  table.append(caption);
  const head = element("thead");
  const headerRow = element("tr");
  const corner = element("th");
  corner.scope = "col";
  corner.setAttribute("aria-label", "Row labels");
  headerRow.append(corner);
  for (const column of grid.columns) {
    const heading = element("th", column);
    heading.scope = "col";
    headerRow.append(heading);
  }
  head.append(headerRow);
  const body = element("tbody");
  const controls = element("div", undefined, "table-controls");
  paged(
    body,
    grid.rows,
    ([rowLabel, cells]) => {
      const tableRow = element("tr");
      const heading = element("th", rowLabel);
      heading.scope = "row";
      tableRow.append(heading);
      for (const cell of cells) {
        const data = element("td");
        data.append(element("code", cell));
        tableRow.append(data);
      }
      return tableRow;
    },
    controls,
  );
  table.append(head, body);
  scroller.append(table);
  section.append(scroller, controls);
  return section;
}

function indexedBodyNode(value: Extract<Value, { kind: "indexed" }>, body: ValueBody): HTMLElement {
  if (body.kind !== "grid" && body.kind !== "slices") {
    return listDetails(
      value.display,
      value.entries.map((entry) => ({ name: entry.display_key, value: entry.value })),
    );
  }
  const details = element("details");
  details.className = "indexed-tables";
  details.append(element("summary", value.display));
  details.addEventListener("toggle", () => {
    if (!details.open || details.childElementCount > 1) return;
    if (body.kind === "grid") {
      details.append(tableNode(body.body));
    } else {
      paged(details, body.body, ([label, grid]) => tableNode(grid, label));
    }
  });
  return details;
}

function valueNode(value: Value): HTMLElement {
  switch (value.kind) {
    case "struct":
      return listDetails(value.display, value.fields);
    case "indexed":
      return listDetails(
        value.display,
        value.entries.map((entry) => ({ name: entry.display_key, value: entry.value })),
      );
    default:
      return element("code", value.display);
  }
}

function resultValueNode(value: Value, body: ValueBody): HTMLElement {
  return value.kind === "indexed" ? indexedBodyNode(value, body) : valueNode(value);
}
export class Output {
  private generation = 0;
  private cleanups: (() => void)[] = [];
  private readonly parent: HTMLElement;
  private readonly focus: (range: SourceRange) => void;
  constructor(parent: HTMLElement, focus: (range: SourceRange) => void) {
    this.parent = parent;
    this.focus = focus;
  }
  clear(message = "Run to see results.") {
    this.generation++;
    this.cleanups.forEach((cleanup) => cleanup());
    this.cleanups = [];
    this.parent.replaceChildren(element("p", message));
  }
  render(outcome: Outcome): string {
    this.clear();
    this.parent.replaceChildren();
    switch (outcome.status) {
      case "rejected":
        this.parent.append(element("p", outcome.error.message, "error"));
        return "Source rejected";
      case "compile_error":
        for (const diagnostic of outcome.diagnostics) {
          const card = element("article", undefined, "diagnostic");
          card.append(
            element("h3", diagnostic.code ?? "Compile error"),
            element("p", diagnostic.message),
          );
          for (const label of diagnostic.labels) {
            const button = element(
              "button",
              `${diagnostic.file}:${label.range.start.line + 1}:${label.range.start.character + 1}${label.message ? ` — ${label.message}` : ""}`,
            );
            button.type = "button";
            button.addEventListener("click", () => this.focus(label.range));
            card.append(button);
          }
          if (diagnostic.help) card.append(element("p", `Hint: ${diagnostic.help}`));
          this.parent.append(card);
        }
        return "Compile error";
      case "evaluated": {
        const evaluation = outcome.evaluation;
        for (const notice of evaluation.notices)
          this.parent.append(
            element(
              "p",
              notice.kind === "plot_error" ? `${notice.name}: ${notice.message}` : notice.message,
              "error",
            ),
          );
        this.parent.append(element("h2", "Values"));
        paged(this.parent, evaluation.values, (declaration) => {
          const row = element("div", undefined, "value-row");
          row.dataset.declarationName = declaration.name;
          const name = element("code", declaration.name);
          name.title = declaration.declaration_kind;
          const result = declaration.outcome;
          row.append(
            name,
            result.status === "value"
              ? resultValueNode(result.value, result.body)
              : element(
                  "span",
                  result.error.kind === "evaluation_failed"
                    ? result.error.message
                    : `Dependency failed: ${result.error.failed_dependencies.join(", ")}`,
                  "error",
                ),
          );
          return row;
        });
        if (!evaluation.values.length) this.parent.append(element("p", "No values were produced."));
        if (evaluation.assertions.length) {
          this.parent.append(element("h2", "Assertions"));
          paged(this.parent, evaluation.assertions, (assertion) =>
            element(
              "p",
              `${assertion.name}: ${assertion.outcome.status.toUpperCase()}${assertion.outcome.status === "pass" ? "" : ` — ${assertion.outcome.message}`}${assertion.affected_declarations.length ? ` (affected: ${assertion.affected_declarations.join(", ")})` : ""}`,
              assertion.outcome.status === "pass" ? "success" : "error",
            ),
          );
        }
        if (evaluation.figures.length) {
          this.parent.append(element("h2", "Plots"));
          const generation = this.generation;
          paged(this.parent, evaluation.figures, (figure) => {
            const container = element("figure");
            container.append(element("figcaption", figure.name));
            const target = element("div");
            container.append(target);
            const current = () => this.generation === generation;
            void renderFigure(target, figure.spec, current)
              .then((cleanup) => {
                if (cleanup) this.cleanups.push(cleanup);
              })
              .catch((error: unknown) => {
                if (current())
                  target.replaceChildren(
                    element(
                      "p",
                      error instanceof Error ? error.message : "Plot rendering failed",
                      "error",
                    ),
                  );
              });
            return container;
          });
        }
        this.parent.append(
          element(
            "p",
            `Graphcal ${evaluation.compiler_version} · running locally in WebAssembly`,
            "muted",
          ),
        );
        return evaluation.has_errors ? "Completed with errors" : "Up to date";
      }
    }
  }
}
