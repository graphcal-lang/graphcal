// Pure selection/projection for the adaptive input outline. Identity paths carry
// constructor steps as well as fields/entries; diagnostic paths remain separate.
(function (global) {
  "use strict";

  function matches(parts, description, query) {
    var text = parts.join(" ").concat(" ", description).toLowerCase();
    return query.toLowerCase().split(/[\s›]+/).filter(Boolean).every(function (word) {
      return text.includes(word);
    });
  }

  function isPathPrefix(prefix, path) {
    return prefix.length <= path.length && prefix.every(function (step, index) {
      return step.kind === path[index].kind && step.index === path[index].index;
    });
  }

  function tree(fields) {
    var root = { groups: new Map(), rows: [] };
    fields.forEach(function (field) {
      var group = root;
      field.labels.slice(0, -1).forEach(function (name) {
        if (!group.groups.has(name)) group.groups.set(name, { name: name, groups: new Map(), rows: [] });
        group = group.groups.get(name);
      });
      group.rows.push(field);
    });
    return root;
  }

  function descendants(group) {
    return group.rows.concat(Array.from(group.groups.values()).flatMap(descendants));
  }

  global.GraphcalReportOutlineState = {
    matches: matches,
    isPathPrefix: isPathPrefix,
    tree: tree,
    descendants: descendants,
  };
})(window);
