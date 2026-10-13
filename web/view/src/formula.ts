// Syntax colouring for the formula bar. The token classes mirror the TextMate
// scopes of vscode-graphcal/syntaxes/graphcal.tmLanguage.json (expression-level
// rules only, in the grammar's precedence order) so the bar reads like the
// editor. Colours live in styles.css under `.tok-*`.

export type TokenClass =
  | "comment"
  | "string"
  | "keyword" // keyword.control: if / else / for / match / table
  | "builtin" // support.function.builtin: scan / unfold
  | "function" // entity.name.function
  | "number" // constant.numeric
  | "unit" // entity.name.type.unit
  | "namespace" // entity.name.namespace
  | "variable" // variable.other (@refs, for-bindings)
  | "variable-punct" // punctuation.definition.variable (the @)
  | "constant" // variable.other.constant (PI, ALL_CAPS)
  | "language" // constant.language: true / false, wildcard
  | "type" // entity.name.type, support.type
  | "accessor" // punctuation.accessor (::, .)
  | "operator" // keyword.operator.*
  | "punctuation" // brackets, comma, colon, semicolon
  | "plain";

export interface Token {
  text: string;
  cls: TokenClass;
}

const ident = "[a-zA-Z][a-zA-Z0-9_]*";
const path = `${ident}(?:\\s*\\.\\s*${ident})*`;
const qualified = `(?:${path}\\s*::\\s*)?${ident}`;
const number = "[0-9][0-9_]*(?:\\.[0-9][0-9_]*)?(?:[eE][+-]?[0-9]+)?";
const unitAtom = `(?:${path}\\s*::\\s*)?${ident}(?:\\^-?[0-9]+(?:/[0-9]+)?)?`;
const unitExpr = `(?:(?:1[ \\t]*/[ \\t]*)?${unitAtom}(?:[ \\t]*[*/][ \\t]*${unitAtom})*|\\([^\\r\\n()]+\\)(?:\\^-?[0-9]+(?:/[0-9]+)?)?)`;
const notKeyword = "(?!(?:if|else|match|for|in|as|true|false)\\b)";

interface Rule {
  re: RegExp;
  emit: (m: RegExpExecArray) => Token[];
}

function one(cls: TokenClass): (m: RegExpExecArray) => Token[] {
  return (m) => [{ text: m[0], cls }];
}

/** Split a qualified name `a.b::c` into namespace / accessor / member tokens. */
function qualifiedTokens(text: string, member: TokenClass): Token[] {
  const at = text.lastIndexOf("::");
  if (at < 0) return [{ text, cls: member }];
  return [
    { text: text.slice(0, at), cls: "namespace" },
    { text: "::", cls: "accessor" },
    { text: text.slice(at + 2), cls: member },
  ];
}

function unitTokens(text: string): Token[] {
  const out: Token[] = [];
  const re = new RegExp(
    `(${path})(\\s*::\\s*)(${ident})|${ident}|-?[0-9]+(?:/[0-9]+)?|[*/^]|[()]|\\s+`,
    "y",
  );
  let i = 0;
  while (i < text.length) {
    re.lastIndex = i;
    const m = re.exec(text);
    if (!m) {
      out.push({ text: text[i] ?? "", cls: "plain" });
      i += 1;
      continue;
    }
    const s = m[0];
    if (m[1] !== undefined) {
      out.push({ text: m[1], cls: "namespace" }, { text: m[2] ?? "::", cls: "accessor" });
      out.push({ text: m[3] ?? "", cls: "unit" });
    } else if (/^[a-zA-Z]/.test(s)) out.push({ text: s, cls: "unit" });
    else if (/^-?[0-9]/.test(s)) out.push({ text: s, cls: "number" });
    else if (/^[*/^]$/.test(s)) out.push({ text: s, cls: "operator" });
    else if (/^[()]$/.test(s)) out.push({ text: s, cls: "punctuation" });
    else out.push({ text: s, cls: "plain" });
    i += s.length;
  }
  return out;
}

const rules: Rule[] = [
  { re: /\/\/.*$/y, emit: one("comment") },
  { re: /"[^"]*(?:"|$)/y, emit: one("string") },
  { re: /\b(?:if|else|for|match|table)\b/y, emit: one("keyword") },
  { re: /\b(?:scan|unfold)\b(?=\s*\()/y, emit: one("builtin") },
  // for-binding: `c: Component` (the colon separates a variable from its type).
  {
    re: new RegExp(`\\b(${ident})(\\s*:\\s*)(?=${ident})`, "y"),
    emit: (m) => [
      { text: m[1] ?? "", cls: "variable" },
      { text: m[2] ?? "", cls: "punctuation" },
    ],
  },
  {
    re: new RegExp(`\\b(${qualified})(?=\\s*(?:<[^>]*>)?\\s*\\()`, "y"),
    emit: (m) => qualifiedTokens(m[1] ?? "", "function"),
  },
  {
    re: new RegExp(`\\b(${number})([ \\t]+)${notKeyword}(${unitExpr})`, "y"),
    emit: (m) => [
      { text: m[1] ?? "", cls: "number" },
      { text: m[2] ?? "", cls: "plain" },
      ...unitTokens(m[3] ?? ""),
    ],
  },
  {
    re: new RegExp(`(@)(${qualified})`, "y"),
    emit: (m) => [{ text: "@", cls: "variable-punct" }, ...qualifiedTokens(m[2] ?? "", "variable")],
  },
  {
    re: new RegExp(
      `\\b([a-z][a-z0-9_]*(?:\\s*\\.\\s*[a-z][a-z0-9_]*)*)(\\s*::\\s*)([A-Z][A-Z0-9_]*)\\b`,
      "y",
    ),
    emit: (m) => [
      { text: m[1] ?? "", cls: "namespace" },
      { text: m[2] ?? "", cls: "accessor" },
      { text: m[3] ?? "", cls: "constant" },
    ],
  },
  { re: /\b(?:true|false)\b/y, emit: one("language") },
  { re: /(?<![a-zA-Z0-9])_(?![a-zA-Z0-9_])/y, emit: one("language") },
  { re: new RegExp(`\\b${number}\\b`, "y"), emit: one("number") },
  {
    re: /\b(?:Dimensionless|Bool|Int|Datetime|Complex|Key|Dim|Index|Nat|Type|Fin)\b/y,
    emit: one("type"),
  },
  { re: /\b(?:[A-Z][A-Z0-9_]*[A-Z0-9]|[A-Z]{2,})\b/y, emit: one("constant") },
  { re: /\b[A-Z][a-z][a-zA-Z0-9_]*\b/y, emit: one("type") },
  {
    re: /=>|->|~=|\+\/-|<=|>=|==|!=|&&|\|\||::|[-+*/^%<>=!]/y,
    emit: (m) => [{ text: m[0], cls: m[0] === "::" ? "accessor" : "operator" }],
  },
  { re: /[()[\]{};,:.]/y, emit: one("punctuation") },
  { re: new RegExp(ident, "y"), emit: one("plain") },
  { re: /\s+/y, emit: one("plain") },
];

/** Tokenise one line of Graphcal expression text. Never throws, never drops text. */
export function tokenize(text: string): Token[] {
  const out: Token[] = [];
  let i = 0;
  scan: while (i < text.length) {
    for (const rule of rules) {
      rule.re.lastIndex = i;
      const m = rule.re.exec(text);
      if (!m || m[0].length === 0) continue;
      out.push(...rule.emit(m));
      i += m[0].length;
      continue scan;
    }
    out.push({ text: text[i] ?? "", cls: "plain" });
    i += 1;
  }
  return out;
}

export function spansOf(tokens: readonly Token[]): HTMLSpanElement[] {
  return tokens.map((t) => {
    const span = document.createElement("span");
    span.className = `tok-${t.cls}`;
    span.textContent = t.text;
    return span;
  });
}

/** `name[row, sub]`: the selected cell, as the editor would colour a reference. */
export function subjectTokens(name: string, labels: readonly string[]): Token[] {
  const out: Token[] = [{ text: name, cls: "variable" }];
  if (labels.length > 0) {
    out.push({ text: "[", cls: "punctuation" });
    labels.forEach((label, i) => {
      if (i > 0) out.push({ text: ", ", cls: "punctuation" });
      out.push(...tokenize(label));
    });
    out.push({ text: "]", cls: "punctuation" });
  }
  return out;
}
