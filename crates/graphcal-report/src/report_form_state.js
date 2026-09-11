// Pure recursive form-state projection for Graphcal report parameter editors.
// DOM events, workers, and evaluation remain in report_runtime.js.
(function (global) {
  "use strict";

  function definitionFor(port, id) {
    for (var i = 0; i < port.definitions.length; i += 1) {
      if (port.definitions[i].id === id) return port.definitions[i];
    }
    return null;
  }

  function axisLabels(axis) {
    if (axis.kind === "named") return axis.variants.slice();
    if (axis.kind === "coordinate") return axis.labels.slice();
    return Array.from({ length: axis.cardinality }, function (_, index) { return "#" + index; });
  }

  // Missing nodes are explicit. Never invent a constructor, zero, false, key,
  // unit, or datetime for a value that evaluation did not provide.
  function emptyDraft(port, schema) {
    if (schema.kind === "algebraic") return { kind: "missing" };
    if (schema.kind === "indexed") {
      return {
        kind: "indexed",
        entries: axisLabels(schema.axis).map(function () {
          return emptyDraft(port, schema.element);
        }),
      };
    }
    return { kind: "literal", expr: "" };
  }

  function draftFromView(port, schema, view) {
    if (schema.kind === "algebraic") {
      if (!view || view.kind !== "struct") return emptyDraft(port, schema);
      var definition = definitionFor(port, schema.definition);
      if (!definition) return null;
      var constructor = definition.constructors.find(function (candidate) {
        return candidate.name === view.type_name;
      });
      if (!constructor) return null;
      var fields = constructor.fields.map(function (field) {
        var viewedField = view.fields.find(function (candidate) { return candidate.name === field.name; });
        return draftFromView(port, field.schema, viewedField && viewedField.value);
      });
      if (fields.some(function (field) { return field === null; })) return null;
      return {
        kind: "algebraic",
        definition: schema.definition,
        constructor: constructor.id,
        fields: fields,
      };
    }
    if (schema.kind === "indexed") {
      if (!view || view.kind !== "indexed") return emptyDraft(port, schema);
      var labels = axisLabels(schema.axis);
      if (view.entries.length !== labels.length) return null;
      return {
        kind: "indexed",
        entries: view.entries.map(function (entry) {
          return draftFromView(port, schema.element, entry.value);
        }),
      };
    }
    if (schema.kind === "key" && schema.axis.kind === "named" && view && view.kind === "label") {
      return { kind: "literal", expr: schema.axis.name + "#" + view.variant };
    }
    return { kind: "literal", expr: view && typeof view.literal === "string" ? view.literal : "" };
  }

  function clone(value) {
    if (!value || value.kind === "missing") return { kind: "missing" };
    if (value.kind === "literal") return { kind: "literal", expr: value.expr };
    if (value.kind === "indexed") {
      return { kind: "indexed", entries: value.entries.map(clone) };
    }
    return {
      kind: "algebraic",
      definition: value.definition,
      constructor: value.constructor,
      fields: value.fields.map(clone),
    };
  }

  function incomplete(value, path) {
    if (!value || value.kind === "missing") {
      return { path: path, message: "Select a constructor to complete this value." };
    }
    if (value.kind === "literal") {
      return value.expr.trim()
        ? null
        : { path: path, message: "Enter a closed value literal before applying." };
    }
    var children = value.kind === "algebraic" ? value.fields : value.entries;
    var segment = value.kind === "algebraic" ? "field" : "entry";
    for (var i = 0; i < children.length; i += 1) {
      var result = incomplete(children[i], path.concat([{ kind: segment, index: i }]));
      if (result) return result;
    }
    return null;
  }

  global.GraphcalReportFormState = {
    axisLabels: axisLabels,
    emptyDraft: emptyDraft,
    draftFromView: draftFromView,
    clone: clone,
    incomplete: incomplete,
  };
})(window);
