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
  "cat"
  "range"
  "import"
  "as"
  "let"
  "if"
  "else"
  "match"
  "for"
  "assert"
  "plot"
  "figure"
  "layer"
  "table"
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

; const G0, const R_EARTH
(const_declaration name: (identifier) @constant)

; dimension Velocity, dimension Length
(dimension_declaration name: (identifier) @type)

; unit km, unit m
(unit_declaration name: (identifier) @type)

; type TransferResult
(type_declaration name: (identifier) @type)

; cat Maneuver
(cat_declaration name: (identifier) @type)

; range TimeStep
(range_declaration name: (identifier) @type)

; fn orbital_velocity
(fn_declaration name: (identifier) @function)

; import "./path.gcl" { name1, name2 as alias2 }
(import_declaration path: (string_literal) @string)

; import nasa/rocket { delta_v }
(import_declaration path: (bare_module_path) @module)

; Casing heuristics for import items:
; ALL_CAPS → constant, PascalCase → type, else → variable
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

; fn params: fn lerp(a: D, b: D)
(fn_param name: (identifier) @variable.parameter)

; fn calls: sqrt(x), ln(x)
(fn_call name: (identifier) @function.call)

; qualified fn calls: module::fn_name(args)
(qualified_fn_call module: (identifier) @module name: (identifier) @function.call)

; ---------------------------------------------------------------
; Graph references: @name, @module::name
; ---------------------------------------------------------------

(graph_ref "@" @operator name: (identifier) @variable)
(graph_ref module: (identifier) @module)

; ---------------------------------------------------------------
; Module imports
; ---------------------------------------------------------------

; import "./path.gcl" as alias;
(import_declaration alias: (identifier) @module)

; Param bindings in instantiated imports: import "path"(name: expr) { ... }
(import_param_binding name: (identifier) @variable)
(import_param_binding ":" @operator)

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

; ---------------------------------------------------------------
; Unfold expression
; ---------------------------------------------------------------

(unfold_expr "unfold" @function.builtin)
(unfold_expr prev: (identifier) @variable.parameter)
(unfold_expr curr: (identifier) @variable.parameter)

; ---------------------------------------------------------------
; Attributes
; ---------------------------------------------------------------

; #[assumes(x, y)], #[expected_fail(Mode::Boost)]
(attribute "#" @punctuation.special)
(attribute "[" @punctuation.special)
(attribute "]" @punctuation.special)
(attribute name: (identifier) @attribute)

; Attribute path arguments: ident, Index::Variant
(attribute_path (identifier) @variable)

; Attribute group arguments: (Index::A, Index::B)
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
; Range declaration — "step" keyword
; ---------------------------------------------------------------

(range_declaration "step" @keyword)

; ---------------------------------------------------------------
; Table expressions
; ---------------------------------------------------------------

; Index names in table[Index1, Index2]: highlighted as types
(table_expr index: (identifier) @type)

; Column headers in table header row: highlighted as index variants
(table_header_row column: (identifier) @constant)

; Row labels in 2D table data rows: highlighted as index variants
(table_data_row row_label: (identifier) @constant)

; Row labels in 1D table data rows: highlighted as index variants
(table_data_row_1d row_label: (identifier) @constant)
