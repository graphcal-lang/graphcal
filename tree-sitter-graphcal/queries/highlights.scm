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
  "base"
  "dim"
  "unit"
  "type"

  "index"
  "linspace"
  "import"
  "include"
  "dag"
  "as"
  "if"
  "else"
  "match"
  "for"
  "assert"
  "plot"
  "figure"
  "layer"
  "table"
  "pub"
] @keyword

; `bind` inside a `pub(bind)` annotation.
(visibility "bind" @keyword)

; ---------------------------------------------------------------
; Literals
; ---------------------------------------------------------------

(number) @number
(boolean) @boolean

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
  "=>"
  "~="
  "+/-"
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

; multi-decl slot names (issue #481)
(multi_decl_slot name: (identifier) @variable)

; dimension Velocity, dimension Length
(dimension_declaration name: (identifier) @type)

; unit km, unit m
(unit_declaration name: (identifier) @type)

; type TransferResult
(type_declaration name: (identifier) @type)

; index Maneuver, index TimeStep
(index_declaration name: (identifier) @type)


; import nasa.rocket.{delta_v}
(import_declaration path: (module_path) @module)

; include nasa.rocket(params).{delta_v}
(include_declaration path: (module_path) @module)

; Highlight individual identifiers in a module path so the
; per-segment color is preserved (Zed's @module style applies to
; the whole path; per-segment refinements live in the children).
(module_path (identifier) @module)

; dag my_pipeline { ... }
(dag_declaration name: (identifier) @function)

; Casing heuristics for import items:
; ALL_CAPS → built-in constant, PascalCase → type, else → variable
(import_item name: (identifier) @constant
  (#match? @constant "^[A-Z][A-Z0-9_]*$"))
(import_item name: (identifier) @type
  (#match? @type "^[A-Z][a-z]"))
(import_item name: (identifier) @variable
  (#match? @variable "^[a-z]"))

(import_item alias: (identifier) @constant
  (#match? @constant "^[A-Z][A-Z0-9_]*$"))
(import_item alias: (identifier) @type
  (#match? @type "^[A-Z][a-z]"))
(import_item alias: (identifier) @variable
  (#match? @variable "^[a-z]"))

; ---------------------------------------------------------------
; Types in annotations
; ---------------------------------------------------------------

; Indexed type index names: Velocity[Maneuver], Mass[Phase, Maneuver]
(indexed_type (identifier) @type)

; Domain constraint names: min, max in Type(min: expr, max: expr)
(type_constraint name: (domain_bound_key) @attribute)

; Builtin type keywords in type positions
(dimensionless) @type.builtin
(bool_type) @type.builtin
(int_type) @type.builtin
(datetime_type) @type.builtin

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


; fn calls: sqrt(x), ln(x). Built-in only — graphcal has no
; user-defined functions, so qualified `module.fn(...)` does not exist.
(fn_call name: (identifier) @function.call)

; ---------------------------------------------------------------
; Graph references: @name, @dag(args).out, @module.dag(args).out
; ---------------------------------------------------------------

(graph_ref "@" @operator name: (identifier) @variable)

; Inline DAG invocation: `@<name>(args).<out>` or
; `@<name>(.<seg>)+ (args).<out>`. The leaf segment of the path (the DAG
; name in call position) is highlighted as a function reference; any
; preceding segments are module aliases brought into scope by `import`;
; the projected output name after the closing `).` is highlighted as a
; variable.
(inline_dag_call "@" @operator)
(inline_dag_call name: (identifier) @function.call !path_segment)
(inline_dag_call name: (identifier) @module path_segment: (identifier) @function.call)
(inline_dag_call output: (identifier) @variable)

; ---------------------------------------------------------------
; Module imports
; ---------------------------------------------------------------

; import nasa.rocket as alias;
(import_declaration alias: (identifier) @module)

; include nasa.rocket(args) as alias;
(include_declaration alias: (identifier) @module)

; Param bindings in include declarations: include "path"(name: expr) { ... }
(include_param_binding name: (identifier) @variable)
(include_param_binding ":" @operator)

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

; Qualified variant: Maneuver.Departure
(qualified_variant index: (identifier) @type variant: (identifier) @constant)

; Union member names: type Foo = A | B;
(union_member name: (identifier) @type)

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
; For comprehension
; ---------------------------------------------------------------

(for_binding var: (identifier) @variable index: (identifier) @type)

; ---------------------------------------------------------------
; Scan expression
; ---------------------------------------------------------------

(scan_expr "scan" @function.builtin)
(scan_expr acc: (identifier) @variable.parameter)
(scan_expr val: (identifier) @variable.parameter)

; ---------------------------------------------------------------
; Unfold expression
; ---------------------------------------------------------------

(unfold_expr "unfold" @function.builtin)
(unfold_expr prev: (identifier) @variable.parameter)
(unfold_expr curr: (identifier) @variable.parameter)

; ---------------------------------------------------------------
; Attributes
; ---------------------------------------------------------------

; #[assumes(x, y)], #[expected_fail(Mode.Boost)]
(attribute "#" @punctuation.special)
(attribute "[" @punctuation.special)
(attribute "]" @punctuation.special)
(attribute name: (identifier) @attribute)

; Attribute path arguments: ident, Index.Variant
(attribute_path (identifier) @variable)

; Attribute group arguments: (Index.A, Index.B)
(attribute_group "(" @punctuation.bracket)
(attribute_group ")" @punctuation.bracket)

; ---------------------------------------------------------------
; Assert declarations
; ---------------------------------------------------------------

(assert_declaration name: (identifier) @variable)

; Tolerance assert operators
(tolerance_assert "~=" @operator)
(tolerance_assert "+/-" @operator)
(tolerance_assert "%" @operator)

; ---------------------------------------------------------------
; Plot declarations
; ---------------------------------------------------------------

(plot_declaration name: (identifier) @variable)
(mark_type) @type
(mark_field "mark" @keyword)
(encode_field "encode" @keyword)
(encode_channel channel: (identifier) @property)
(plot_field name: (identifier) @property)

; ---------------------------------------------------------------
; Figure declarations
; ---------------------------------------------------------------

(figure_declaration name: (identifier) @variable)
(figure_named_field name: (identifier) @property)

; ---------------------------------------------------------------
; Layer declarations
; ---------------------------------------------------------------

(layer_declaration name: (identifier) @variable)
(layer_named_field name: (identifier) @property)

; ---------------------------------------------------------------
; Index declaration — "step" keyword in linspace
; ---------------------------------------------------------------

(index_declaration "step" @keyword)

; ---------------------------------------------------------------
; Table expressions
; ---------------------------------------------------------------

; Index names in table[Index1, Index2]: highlighted as types
(table_expr index: (identifier) @type)

; Column headers in table header row: highlighted as index variants
(table_header_row column: (identifier) @constant)

; Row labels in table data rows: highlighted as index variants
(table_data_row row_label: (identifier) @constant)

; Multi-decl (issue #481) surface form — mirror single-decl highlights.

; Shared axis names in table[I1, I2, ..., (slots)]: highlighted as types
(multi_table_expr shared_axis: (identifier) @type)

; Extra-axis names inside the slot tuple `(_, _, ExtraAxis)`: highlighted
; as types. (`_` placeholders are a literal token, not an identifier.)
(slot_axis_entry (identifier) @type)

; Header-row cells that are bare variant identifiers: index variants.
; (`_` placeholders are a literal token; qualified `Axis::Variant` is
; already covered by the `qualified_variant` rule above.)
(multi_header_cell (identifier) @constant)

; Row labels in multi-decl data rows.
(multi_data_row row_label: (identifier) @constant)
