; highlights.scm — Graphcal syntax highlighting queries
; Compatible with Zed, Neovim, and Helix (minor per-editor tweaks may be needed)

; ---------------------------------------------------------------
; Comments
; ---------------------------------------------------------------

(line_comment) @comment

; ---------------------------------------------------------------
; Keywords
; ---------------------------------------------------------------

[
  "param"
  "node"
  "const"
  "dimension"
  "unit"
  "type"
  "fn"
  "index"
  "use"
  "let"
  "if"
  "else"
  "match"
  "for"
] @keyword

; ---------------------------------------------------------------
; Literals
; ---------------------------------------------------------------

(number) @number
(boolean) @boolean
(string_literal) @string

; ---------------------------------------------------------------
; Operators
; ---------------------------------------------------------------

[
  "+"
  "-"
  "*"
  "/"
  "^"
  "%"
  "="
  "=="
  "!="
  "<"
  ">"
  "<="
  ">="
  "&&"
  "||"
  "!"
  "->"
  "::"
  "=>"
] @operator

; ---------------------------------------------------------------
; Punctuation
; ---------------------------------------------------------------

[ "(" ")" ] @punctuation.bracket
[ "{" "}" ] @punctuation.bracket
[ "[" "]" ] @punctuation.bracket

[ ";" "," ":" "." "|" ] @punctuation.delimiter

; ---------------------------------------------------------------
; Declarations — names
; ---------------------------------------------------------------

; param dry_mass, node v_exhaust
(param_declaration name: (identifier) @variable)
(node_declaration name: (identifier) @variable)

; const G0, const R_EARTH
(const_declaration name: (identifier) @constant)

; dimension Velocity, dimension Length
(dimension_declaration name: (identifier) @type)

; unit km, unit m
(unit_declaration name: (identifier) @type)

; type TransferResult
(type_declaration name: (identifier) @type)

; index Maneuver
(index_declaration name: (identifier) @type)

; fn orbital_velocity
(fn_declaration name: (identifier) @function)

; use "./path.gcl" { name1, name2 }
(use_declaration path: (string_literal) @string)

; ---------------------------------------------------------------
; Types in annotations
; ---------------------------------------------------------------

; Dimensionless keyword in type positions
(dimensionless) @type.builtin

; Generic constraints: Dim, Index
(generic_constraint) @type.builtin

; Generic parameter names: D, I
(generic_param name: (identifier) @type)

; Dimension terms in type annotations (Length, Time, Mass, etc.)
(dim_term (identifier) @type)

; Unit terms in unit expressions
(unit_term (identifier) @type)

; ---------------------------------------------------------------
; Function calls and definitions
; ---------------------------------------------------------------

; fn params: fn lerp(a: D, b: D)
(fn_param name: (identifier) @variable.parameter)

; fn calls: sqrt(x), ln(x)
(fn_call name: (identifier) @function.call)

; ---------------------------------------------------------------
; Graph references: @name
; ---------------------------------------------------------------

(graph_ref "@" @operator name: (identifier) @variable)

; ---------------------------------------------------------------
; Struct and index usage
; ---------------------------------------------------------------

; Struct construction: TransferResult { ... }
(struct_construction type: (identifier) @type)

; Field access: @transfer.dv1
(field_access field: (identifier) @property)

; Field declarations in type: dv1: Velocity
(field_declaration name: (identifier) @property)

; Field init shorthand/explicit: dv1, dv1: expr
(field_init name: (identifier) @property)

; Qualified variant: Maneuver::Departure
(qualified_variant index: (identifier) @type variant: (identifier) @constant)

; Variant declarations in tagged unions: Impulsive { delta_v: Velocity }
(variant_declaration name: (identifier) @type)

; Index declaration variants: { Departure, Correction, Insertion }
(variant (identifier) @constant)

; ---------------------------------------------------------------
; Match expressions
; ---------------------------------------------------------------

; Match pattern variant name: Impulsive { ... } =>
(match_pattern variant: (identifier) @type)

; Wildcard pattern: _
(wildcard) @variable.builtin

; Pattern binding name (shorthand): { thrust }
; Pattern binding with rename: { name: binding }
(pattern_binding name: (identifier) @property)

; ---------------------------------------------------------------
; Let bindings
; ---------------------------------------------------------------

(let_binding name: (identifier) @variable)

; ---------------------------------------------------------------
; For comprehension
; ---------------------------------------------------------------

(for_binding var: (identifier) @variable index: (identifier) @type)

; ---------------------------------------------------------------
; Scan expression
; ---------------------------------------------------------------

(scan_expr "scan" @function.builtin)
(scan_expr acc: (identifier) @variable.parameter)
(scan_expr val: (identifier) @variable.parameter)
