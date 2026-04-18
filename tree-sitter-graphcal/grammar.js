/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

// Precedence levels (lowest to highest), matching the Graphcal parser.
const PREC = {
  CONVERT: 1,    // ->
  OR: 2,         // ||
  AND: 3,        // &&
  COMPARE: 4,    // == != < > <= >=
  ADD: 5,        // + -
  MUL: 6,        // * /
  UNARY: 7,      // - !
  POWER: 8,      // ^
  POSTFIX: 9,    // . []
  CALL: 10,      // fn(...)
};

module.exports = grammar({
  name: "graphcal",

  extras: $ => [
    /\s/,
    $.line_comment,
  ],

  word: $ => $.identifier,

  conflicts: $ => [
    // `identifier {` could be a struct_construction or a bare identifier
    // followed by a brace body (e.g., in `if condition { ... }`).
    [$._primary_expr, $.struct_construction],
    // `identifier < type_expr` could be fn_call turbofish or struct_construction type args.
    [$.generic_arg, $.struct_construction],
  ],

  rules: {
    source_file: $ => repeat($._declaration),

    // ---------------------------------------------------------------
    // Declarations
    // ---------------------------------------------------------------

    // Visibility annotation: `pub` or `pub(bind)`.
    // `bind` is a contextual keyword parsed only inside the parens after `pub`.
    visibility: $ => seq("pub", optional(seq("(", "bind", ")"))),

    _declaration: $ => choice(
      $.param_declaration,
      $.node_declaration,
      $.dimension_declaration,
      $.unit_declaration,
      $.type_declaration,

      $.index_declaration,
      $.import_declaration,
      $.include_declaration,
      $.dag_declaration,
      $.assert_declaration,
      $.plot_declaration,
      $.figure_declaration,
      $.layer_declaration,
    ),

    // #[name] or #[name(arg1, arg2)] or #[name(Index::Variant, (A::X, B::Y))]
    attribute: $ => seq(
      "#",
      "[",
      field("name", $.identifier),
      optional(seq(
        "(",
        optional(seq(
          $._attribute_arg,
          repeat(seq(",", $._attribute_arg)),
          optional(","),
        )),
        ")",
      )),
      "]",
    ),

    // An attribute argument: a path (ident or ident::ident::...) or a group ((arg, arg, ...))
    _attribute_arg: $ => choice(
      $.attribute_path,
      $.attribute_group,
    ),

    attribute_path: $ => seq(
      $.identifier,
      repeat(seq("::", $.identifier)),
    ),

    attribute_group: $ => seq(
      "(",
      $._attribute_arg,
      repeat(seq(",", $._attribute_arg)),
      optional(","),
      ")",
    ),

    // param dry_mass: Mass = 1200 kg;
    // param dry_mass: Mass;  (required param, no default)
    param_declaration: $ => seq(
      repeat($.attribute),
      optional($.visibility),
      "param",
      field("name", $.identifier),
      optional(seq(":", field("type", $.type_expr))),
      optional(seq("=", field("value", $._expr))),
      ";",
    ),

    // node v_exhaust: Velocity = @isp * @g0;
    // const node g0: Acceleration = 9.80665 m/s^2;
    node_declaration: $ => seq(
      repeat($.attribute),
      optional($.visibility),
      optional("const"),
      "node",
      field("name", $.identifier),
      optional(seq(":", field("type", $.type_expr))),
      "=",
      field("value", $._expr),
      ";",
    ),

    // base dim Length;
    // dim D;                            -- required dim (bound via include)
    // dim Velocity = Length / Time;
    dimension_declaration: $ => seq(
      optional($.visibility),
      optional("base"),
      "dim",
      field("name", $.identifier),
      optional(seq("=", field("definition", $.dim_expr))),
      ";",
    ),

    // base unit m: Length;                 -- base unit (no body)
    // unit km: Length = 1000 m;             -- derived unit
    // const unit hr: Time = 3600 s;         -- compile-time-only unit
    unit_declaration: $ => choice(
      seq(
        optional($.visibility),
        "base",
        "unit",
        field("name", $.identifier),
        ":",
        field("dimension", $.dim_expr),
        ";",
      ),
      seq(
        optional($.visibility),
        optional("const"),
        "unit",
        field("name", $.identifier),
        ":",
        field("dimension", $.dim_expr),
        "=",
        field("definition", $.unit_def),
        ";",
      ),
    ),

    // type TransferResult { dv1: Velocity, dv2: Velocity }  -- record type
    // type Eci {}                                            -- empty record / marker
    // type Element;                                          -- required type (bound via include)
    // type ManeuverKind = Impulsive | Coasting;              -- union type
    // type Result<D: Dim> = Ok<D> | Err;                     -- generic union type
    type_declaration: $ => seq(
      repeat($.attribute),
      optional($.visibility),
      "type",
      field("name", $.identifier),
      optional(field("generics", $.generic_params)),
      choice(
        // Record type: type Foo { field: Type, ... } or type Foo {}
        seq(
          "{",
          optional(seq(
            $.field_declaration,
            repeat(seq(",", $.field_declaration)),
            optional(","),
          )),
          "}",
        ),
        // Unit type: type Foo;
        ";",
        // Union type: type Foo = A | B | C;
        seq("=", $.union_members, ";"),
      ),
    ),

    field_declaration: $ => seq(
      field("name", $.identifier),
      ":",
      field("type", $.type_expr),
    ),

    // Union members: A | B or A<D> | B
    union_members: $ => seq(
      $.union_member,
      repeat1(seq("|", $.union_member)),
    ),

    // A single union member, optionally with type arguments: Ok<D>
    union_member: $ => seq(
      field("name", $.identifier),
      optional(seq(
        "<",
        field("type_arg", $.type_expr),
        repeat(seq(",", field("type_arg", $.type_expr))),
        optional(","),
        ">",
      )),
    ),

    // index Maneuver = { Departure, Correction, Insertion };
    // index TimeStep = linspace(0.0 s, 1.0 s, step: 0.1 s);
    // index Foo;  (required named)
    // index Foo: Time;  (required range)
    index_declaration: $ => choice(
      // Named index: index Maneuver = { Departure, Correction, Insertion };
      seq(
        optional($.visibility),
        "index",
        field("name", $.identifier),
        "=",
        "{",
        optional(seq(
          $.variant,
          repeat(seq(",", $.variant)),
          optional(","),
        )),
        "}",
        ";",
      ),
      // Linspace index: index TimeStep = linspace(0.0 s, 1.0 s, step: 0.1 s);
      seq(
        optional($.visibility),
        "index",
        field("name", $.identifier),
        "=",
        "linspace",
        "(",
        field("start", $._expr),
        ",",
        field("end", $._expr),
        ",",
        "step",
        ":",
        field("step", $._expr),
        ")",
        ";",
      ),
      // Required named: index Foo;
      seq(optional($.visibility), "index", field("name", $.identifier), ";"),
      // Required range: index Foo: Time;
      seq(
        optional($.visibility),
        "index",
        field("name", $.identifier),
        ":",
        field("dimension", $.dim_expr),
        ";",
      ),
    ),

    variant: $ => $.identifier,


    generic_params: $ => seq(
      "<",
      $.generic_param,
      repeat(seq(",", $.generic_param)),
      optional(","),
      ">",
    ),

    generic_param: $ => seq(
      field("name", $.identifier),
      ":",
      field("constraint", $.generic_constraint),
      optional(seq("=", field("default", $.type_expr))),
    ),

    generic_constraint: $ => choice("Dim", "Index", "Nat", "Type"),


    // import "./path.gcl" { name1, name2 as alias2 };  -- selective import
    // import "./path.gcl";                               -- module import (name from filename)
    // import "./path.gcl" as alias;                      -- module import with alias
    // import nasa/rocket { delta_v };                     -- bare module path
    // import nasa/rocket as r;                            -- bare module path with alias
    import_declaration: $ => seq(
      repeat($.attribute),
      optional($.visibility),
      "import",
      field("path", choice($.string_literal, $.bare_module_path, $.parent_scope_path)),
      choice(
        // Selective import: { name1, name2 as alias }
        seq(
          "{",
          optional(seq(
            $.import_item,
            repeat(seq(",", $.import_item)),
            optional(","),
          )),
          "}",
          ";",
        ),
        // Module import with alias: as name ;
        seq("as", field("alias", $.identifier), ";"),
        // Module import (bare): ;
        ";",
      ),
    ),

    // include "./rocket.gcl"(dry_mass: 800.0 kg) { delta_v };  -- selective include
    // include "./rocket.gcl"(dry_mass: 800.0 kg) as stage_1;   -- module include
    // include "./rocket.gcl" { delta_v };                        -- include without params
    // include nasa/rocket(dry_mass: 800.0 kg) as stage;          -- bare module path
    // include my_dag(x: 1.0) { result };                          -- inline DAG reference
    include_declaration: $ => seq(
      repeat($.attribute),
      optional($.visibility),
      "include",
      field("path", choice($.string_literal, $.bare_module_path, $.dag_ref_path, $.parent_scope_path)),
      optional(field("param_bindings", $.include_param_bindings)),
      choice(
        // Selective: { name1, name2 as alias }
        seq(
          "{",
          optional(seq(
            $.import_item,
            repeat(seq(",", $.import_item)),
            optional(","),
          )),
          "}",
          ";",
        ),
        // Module with alias: as name ;
        seq("as", field("alias", $.identifier), ";"),
        // Module (bare): ;
        ";",
      ),
    ),

    // Param bindings for include instantiation: (name: expr, ...)
    include_param_bindings: $ => seq(
      "(",
      $.include_param_binding,
      repeat(seq(",", $.include_param_binding)),
      optional(","),
      ")",
    ),

    // A single param binding in include: name: expr
    include_param_binding: $ => seq(
      field("name", $.identifier),
      ":",
      field("value", $._expr),
    ),

    // dag name { declarations... }
    dag_declaration: $ => seq(
      repeat($.attribute),
      optional($.visibility),
      "dag",
      field("name", $.identifier),
      "{",
      repeat($._declaration),
      "}",
    ),

    // Bare module path: nasa/rocket, nasa/orbital/transfer
    // Requires at least two segments separated by /
    bare_module_path: $ => seq(
      $.identifier,
      repeat1(seq("/", $.identifier)),
    ),

    // Inline DAG reference path: a single identifier (e.g., `my_dag`)
    // Used in `include` to reference inline DAG definitions.
    dag_ref_path: $ => $.identifier,

    // Parent scope path: `..` or `../..` etc.
    // Used in `import` inside DAG blocks to access the enclosing scope.
    parent_scope_path: $ => seq(
      "..",
      repeat(seq("/", "..")),
    ),

    // Import item with optional alias: name or name as alias
    import_item: $ => seq(
      repeat($.attribute),
      field("name", $.identifier),
      optional(seq("as", field("alias", $.identifier))),
    ),

    // assert velocity_in_range = @velocity < @max_velocity;
    // assert mass_approx = @mass ~= 100.0 kg +/- 1.0 kg;
    // assert approx_pct = @x ~= 50.0 +/- 5 %;
    assert_declaration: $ => seq(
      repeat($.attribute),
      optional($.visibility),
      "assert",
      field("name", $.identifier),
      "=",
      field("body", $.assert_body),
      ";",
    ),

    // plot mass_vs_dv = {
    //     mark: point,
    //     encode: {
    //         x: for m: Maneuver { @delta_v[m] },
    //         y: for m: Maneuver { @spacecraft_mass[m] },
    //     },
    //     title: "Spacecraft Mass vs Delta-V",
    // };
    plot_declaration: $ => seq(
      repeat($.attribute),
      optional($.visibility),
      "plot",
      field("name", $.identifier),
      "=",
      "{",
      optional(seq(
        $._plot_body_field,
        repeat(seq(",", $._plot_body_field)),
        optional(","),
      )),
      "}",
      ";",
    ),

    _plot_body_field: $ => choice(
      $.mark_field,
      $.encode_field,
      $.plot_field,
    ),

    // mark: point, or mark: line { stroke_width: 2.0, },
    mark_field: $ => seq(
      "mark",
      ":",
      field("mark_type", $.mark_type),
      optional(seq(
        "{",
        optional(seq(
          $.plot_field,
          repeat(seq(",", $.plot_field)),
          optional(","),
        )),
        "}",
      )),
    ),

    mark_type: $ => choice("point", "line", "bar", "area", "rect", "tick"),

    // encode: { x: expr, y: expr, color: expr, ... },
    encode_field: $ => seq(
      "encode",
      ":",
      "{",
      optional(seq(
        $.encode_channel,
        repeat(seq(",", $.encode_channel)),
        optional(","),
      )),
      "}",
    ),

    encode_channel: $ => seq(
      field("channel", $.identifier),
      ":",
      field("value", $._expr),
    ),

    plot_field: $ => seq(
      field("name", $.identifier),
      ":",
      field("value", $._expr),
    ),

    // figure comparison = {
    //     plots: [curve_a, curve_b],
    //     title: "Side-by-side Comparison",
    // };
    figure_declaration: $ => seq(
      repeat($.attribute),
      optional($.visibility),
      "figure",
      field("name", $.identifier),
      "=",
      "{",
      optional(seq(
        $.figure_field,
        repeat(seq(",", $.figure_field)),
        optional(","),
      )),
      "}",
      ";",
    ),

    figure_field: $ => choice(
      $.figure_plots_field,
      $.figure_named_field,
    ),

    // plots: [name1, name2]
    figure_plots_field: $ => seq(
      "plots",
      ":",
      "[",
      optional(seq(
        $.identifier,
        repeat(seq(",", $.identifier)),
        optional(","),
      )),
      "]",
    ),

    // title: "...", or other key: value fields
    figure_named_field: $ => seq(
      field("name", $.identifier),
      ":",
      field("value", $._expr),
    ),

    // layer decay_with_points = {
    //     plots: [line_layer, point_layer],
    //     title: "Decay Curve with Points",
    // };
    layer_declaration: $ => seq(
      repeat($.attribute),
      optional($.visibility),
      "layer",
      field("name", $.identifier),
      "=",
      "{",
      optional(seq(
        $.layer_field,
        repeat(seq(",", $.layer_field)),
        optional(","),
      )),
      "}",
      ";",
    ),

    layer_field: $ => choice(
      $.layer_plots_field,
      $.layer_named_field,
    ),

    // plots: [name1, name2]
    layer_plots_field: $ => seq(
      "plots",
      ":",
      "[",
      optional(seq(
        $.identifier,
        repeat(seq(",", $.identifier)),
        optional(","),
      )),
      "]",
    ),

    // title: "...", or other key: value fields
    layer_named_field: $ => seq(
      field("name", $.identifier),
      ":",
      field("value", $._expr),
    ),

    assert_body: $ => choice(
      $.tolerance_assert,
      $._expr,
    ),

    // actual ~= expected +/- tolerance
    // actual ~= expected +/- tolerance %
    tolerance_assert: $ => seq(
      field("actual", $._expr),
      "~=",
      field("expected", $._expr),
      "+/-",
      field("tolerance", $._expr),
      optional("%"),
    ),

    // ---------------------------------------------------------------
    // Type expressions
    // ---------------------------------------------------------------

    type_expr: $ => choice(
      $.indexed_type,
      $.constrained_type,
      $._type_expr_base,
    ),

    _type_expr_base: $ => choice(
      $.dimensionless,
      $.bool_type,
      $.int_type,
      $.datetime_type,
      $.type_application,
      $.dim_expr,
    ),

    // Constrained type: Mass(min: 100 kg, max: 2000 kg)
    constrained_type: $ => seq(
      field("base", $._type_expr_base),
      $.type_constraints,
    ),

    type_constraints: $ => seq(
      "(",
      $.type_constraint,
      repeat(seq(",", $.type_constraint)),
      optional(","),
      ")",
    ),

    type_constraint: $ => seq(
      field("name", alias(choice("min", "max"), $.domain_bound_key)),
      ":",
      field("value", $._expr),
    ),

    domain_bound_key: _$ => choice("min", "max"),

    // Generic type application: Vec3<Length, ECI>
    // Uses dynamic precedence to prefer type_application over parsing `<` as
    // a comparison operator when an identifier is followed by `<` in type context.
    type_application: $ => prec.dynamic(2, seq(
      field("name", $.identifier),
      "<",
      field("type_arg", $.type_expr),
      repeat(seq(",", field("type_arg", $.type_expr))),
      optional(","),
      ">",
    )),

    dimensionless: $ => "Dimensionless",
    bool_type: $ => "Bool",
    int_type: $ => "Int",
    datetime_type: $ => "Datetime",

    // Indexed type: Velocity[Maneuver], Dimensionless[3, 4], D[M, N]
    indexed_type: $ => seq(
      field("base", choice($.constrained_type, $._type_expr_base)),
      "[",
      $._index_expr,
      repeat(seq(",", $._index_expr)),
      "]",
    ),

    // An expression in index position: a name, integer literal, nat addition, or nat multiplication
    _index_expr: $ => choice(
      $.nat_add_expr,
      $.nat_mul_expr,
      $.identifier,
      $.nat_literal,
    ),

    // Nat addition expression in index position: N + 1, M + N + 2, M * N + 1
    nat_add_expr: $ => prec.left(PREC.ADD, seq(
      field("left", choice($.identifier, $.nat_literal, $.nat_add_expr, $.nat_mul_expr)),
      "+",
      field("right", choice($.identifier, $.nat_literal, $.nat_mul_expr)),
    )),

    // Nat multiplication expression in index position: M * N, M * N * P, 2 * N
    nat_mul_expr: $ => prec.left(PREC.MUL, seq(
      field("left", choice($.identifier, $.nat_literal, $.nat_mul_expr)),
      "*",
      field("right", choice($.identifier, $.nat_literal)),
    )),

    // Integer literal in type/index position (e.g., 3 in D[3])
    nat_literal: $ => /[0-9]+/,

    // ---------------------------------------------------------------
    // Dimension expressions: Length, Length^2, Mass * Length / Time^2
    // ---------------------------------------------------------------

    dim_expr: $ => prec.right(PREC.MUL + 1, seq(
      $.dim_term,
      repeat(seq(choice("*", "/"), $.dim_term)),
    )),

    dim_term: $ => prec.right(PREC.POWER + 1, choice(
      seq($.identifier, optional(seq("^", $.number))),
      seq("(", $.dim_expr, ")", optional(seq("^", $.number))),
    )),

    // ---------------------------------------------------------------
    // Unit expressions: m, m/s^2, kg * m / s^2
    // ---------------------------------------------------------------

    unit_expr: $ => prec.right(PREC.MUL + 1, seq(
      $.unit_term,
      repeat(seq(choice("*", "/"), $.unit_term)),
    )),

    unit_term: $ => prec.right(PREC.POWER + 1, choice(
      seq($.identifier, optional(seq("^", $.number))),
      seq("(", $.unit_expr, ")", optional(seq("^", $.number))),
    )),

    // Unit definition in unit declaration: 1000 m, 1 kg * m / s^2
    // Also supports dynamic scale: (@rate) USD
    unit_def: $ => seq(
      field("scale", choice($.number, $.parenthesized_expr)),
      $.unit_expr,
    ),

    // ---------------------------------------------------------------
    // Expressions
    // ---------------------------------------------------------------

    _expr: $ => choice(
      $.binary_expr,
      $.unary_expr,
      $.convert_expr,
      $.as_cast_expr,
      $.if_expr,
      $.match_expr,
      $.for_expr,
      $.scan_expr,
      $.unfold_expr,
      $.table_expr,
      $._postfix_expr,
    ),

    // Conversion: expr -> unit_expr
    convert_expr: $ => prec.left(PREC.CONVERT, seq(
      field("value", $._expr),
      "->",
      field("target", $.unit_expr),
    )),

    // Phantom type cast: expr as TypeExpr
    // Uses _type_expr_base (not type_expr) to avoid ambiguity with index_access [...]
    //
    // Two forms:
    // 1. Generic: `expr as Vec3<Length, Body>` — the `as` keyword followed by
    //    `Ident <` is always parsed as a type_application (not comparison).
    //    This is safe because comparison after `as` makes no semantic sense.
    // 2. Non-generic: `expr as SomeType`
    as_cast_expr: $ => choice(
      // Generic type target — inlined type_application sequence after `as`
      // so that `<` is unambiguously part of the type, not a comparison.
      prec.left(PREC.CONVERT, seq(
        field("value", $._expr),
        "as",
        field("target_type", alias($.as_type_application, $.type_application)),
      )),
      // Non-generic type target
      prec.left(PREC.CONVERT, seq(
        field("value", $._expr),
        "as",
        field("target_type", choice($.dimensionless, $.bool_type, $.int_type, $.dim_expr)),
      )),
    ),

    // Type application used exclusively in as-cast context.
    // Uses high precedence to ensure the parser prefers shifting `<` as part
    // of the type application rather than reducing identifier → dim_term.
    as_type_application: $ => prec(PREC.POWER + 2, seq(
      field("name", $.identifier),
      "<",
      field("type_arg", $.type_expr),
      repeat(seq(",", field("type_arg", $.type_expr))),
      optional(","),
      ">",
    )),

    binary_expr: $ => choice(
      prec.left(PREC.OR, seq(field("left", $._expr), "||", field("right", $._expr))),
      prec.left(PREC.AND, seq(field("left", $._expr), "&&", field("right", $._expr))),
      prec.left(PREC.COMPARE, seq(field("left", $._expr), "==", field("right", $._expr))),
      prec.left(PREC.COMPARE, seq(field("left", $._expr), "!=", field("right", $._expr))),
      prec.left(PREC.COMPARE, seq(field("left", $._expr), "<", field("right", $._expr))),
      prec.left(PREC.COMPARE, seq(field("left", $._expr), ">", field("right", $._expr))),
      prec.left(PREC.COMPARE, seq(field("left", $._expr), "<=", field("right", $._expr))),
      prec.left(PREC.COMPARE, seq(field("left", $._expr), ">=", field("right", $._expr))),
      prec.left(PREC.ADD, seq(field("left", $._expr), "+", field("right", $._expr))),
      prec.left(PREC.ADD, seq(field("left", $._expr), "-", field("right", $._expr))),
      prec.left(PREC.MUL, seq(field("left", $._expr), "*", field("right", $._expr))),
      prec.left(PREC.MUL, seq(field("left", $._expr), "/", field("right", $._expr))),
      prec.left(PREC.MUL, seq(field("left", $._expr), "%", field("right", $._expr))),
      prec.right(PREC.POWER, seq(field("left", $._expr), "^", field("right", $._expr))),
    ),

    unary_expr: $ => prec(PREC.UNARY, seq(
      field("operator", choice("-", "!")),
      field("operand", $._expr),
    )),

    if_expr: $ => prec.right(seq(
      "if",
      field("condition", $._expr),
      field("then", $.brace_body),
      "else",
      field("else", $.brace_body),
    )),

    // match @maneuver { Impulsive { delta_v } => ..., Coasting => ... }
    match_expr: $ => seq(
      "match",
      field("scrutinee", choice(
        $._expr,
        seq(
          "(",
          field("tuple_scrutinee", $._expr),
          repeat1(seq(",", field("tuple_scrutinee", $._expr))),
          ")",
        ),
      )),
      "{",
      optional(seq(
        choice($.match_arm, $.tuple_match_arm),
        repeat(seq(",", choice($.match_arm, $.tuple_match_arm))),
        optional(","),
      )),
      "}",
    ),

    match_arm: $ => seq(
      field("pattern", $.match_pattern),
      "=>",
      field("body", $._expr),
    ),

    tuple_match_arm: $ => seq(
      field("pattern", $.tuple_match_pattern),
      "=>",
      field("body", $._expr),
    ),

    match_pattern: $ => choice(
      // Qualified variant pattern for index match: Maneuver::Departure
      seq(
        field("index", $.identifier),
        "::",
        field("variant", $.identifier),
      ),
      // Bare variant pattern for tagged union match: Variant { bindings }
      seq(
        field("variant", $.identifier),
        optional(seq(
          "{",
          optional(seq(
            $.pattern_binding,
            repeat(seq(",", $.pattern_binding)),
            optional(","),
          )),
          "}",
        )),
      ),
    ),

    tuple_match_pattern: $ => choice(
      "_",
      seq(
        "(",
        field("value", $._expr),
        repeat1(seq(",", field("value", $._expr))),
        ")",
      ),
    ),

    pattern_binding: $ => choice(
      // field_name: _  (wildcard)
      seq(field("name", $.identifier), ":", $.wildcard),
      // field_name: var_name  (rename)
      seq(field("name", $.identifier), ":", field("binding", $.identifier)),
      // field_name  (shorthand: bind to same name)
      field("name", $.identifier),
    ),

    wildcard: $ => "_",

    // for m: Maneuver { ... }
    for_expr: $ => seq(
      "for",
      $.for_binding,
      repeat(seq(",", $.for_binding)),
      "{",
      optional(seq(
        "(",
        field("key_var", $.identifier),
        repeat1(seq(",", field("key_var", $.identifier))),
        ")",
        "=>",
      )),
      field("body", $._expr),
      "}",
    ),

    for_binding: $ => seq(
      field("var", $.identifier),
      ":",
      field("index", choice($.identifier, $.range_expr)),
    ),

    // range(N) expression in for bindings
    range_expr: $ => seq(
      "range",
      "(",
      field("arg", choice($.nat_add_expr, $.identifier, $.nat_literal)),
      ")",
    ),

    // scan(source, init, |acc, val| body) -- accumulator scan (prefix scan)
    scan_expr: $ => seq(
      "scan",
      "(",
      field("source", $._expr),
      ",",
      field("init", $._expr),
      ",",
      "|",
      field("acc", $.identifier),
      ",",
      field("val", $.identifier),
      "|",
      field("body", $._expr),
      ")",
    ),

    // unfold(init, |prev_i, i| body) -- unfold (anamorphism)
    unfold_expr: $ => seq(
      "unfold",
      "(",
      field("init", $._expr),
      ",",
      "|",
      field("prev", $.identifier),
      ",",
      field("curr", $.identifier),
      "|",
      field("body", $._expr),
      ")",
    ),

    // Table expression: table[Index1, Index2] { ... }
    // Syntax sugar for map literals with spreadsheet-like layout.
    // Index specs are named identifiers or integer literals (Nat range).
    table_expr: $ => seq(
      "table",
      "[",
      field("index", choice($.identifier, $.nat_literal)),
      repeat(seq(",", field("index", choice($.identifier, $.nat_literal)))),
      "]",
      "{",
      $.table_body,
      "}",
    ),

    table_body: $ => choice(
      // 3D+: slice sections
      repeat1($.table_slice_section),
      // 1D or 2D: optional header + data rows
      $.table_single,
    ),

    table_slice_section: $ => seq(
      "[",
      $.table_slice_label,
      repeat(seq(",", $.table_slice_label)),
      "]",
      $.table_single,
    ),

    // Slice labels: `Index::Variant` (named axis) or `#N` (Nat range axis).
    table_slice_label: $ => choice(
      $.qualified_variant,
      seq("#", $.nat_literal),
    ),

    table_single: $ => seq(
      optional($.table_header_row),
      repeat1($.table_data_row),
    ),

    // Header row now requires a leading `:` prefix.
    // Omitted when the column axis is a Nat range.
    table_header_row: $ => seq(
      ":",
      field("column", $.identifier),
      repeat(seq(",", field("column", $.identifier))),
      ";",
    ),

    // Data row: `Label: val, val, ...;` for named row axes, or
    // `val, val, ...;` for Nat range row axes. A row with a single
    // value and no label also covers the 1D case.
    table_data_row: $ => seq(
      optional(seq(field("row_label", $.identifier), ":")),
      field("value", $._expr),
      repeat(seq(",", field("value", $._expr))),
      ";",
    ),

    // Postfix expressions: field access, index access, function calls
    _postfix_expr: $ => choice(
      $.field_access,
      $.index_access,
      $.fn_call,
      $.qualified_fn_call,
      $._primary_expr,
    ),

    field_access: $ => prec.left(PREC.POSTFIX, seq(
      field("object", $._expr),
      ".",
      field("field", $.identifier),
    )),

    index_access: $ => prec.left(PREC.POSTFIX, seq(
      field("object", $._expr),
      "[",
      $.index_arg,
      repeat(seq(",", $.index_arg)),
      "]",
    )),

    index_arg: $ => $._expr,

    // Maneuver::Departure
    qualified_variant: $ => seq(
      field("index", $.identifier),
      "::",
      field("variant", $.identifier),
    ),

    fn_call: $ => prec(PREC.CALL, seq(
      field("name", $.identifier),
      optional(seq(
        "<",
        field("generic_arg", $.generic_arg),
        repeat(seq(",", field("generic_arg", $.generic_arg))),
        optional(","),
        ">",
      )),
      "(",
      optional(seq(
        $._expr,
        repeat(seq(",", $._expr)),
        optional(","),
      )),
      ")",
    )),

    // module::fn_name(args) — qualified function call
    qualified_fn_call: $ => prec(PREC.CALL, seq(
      field("module", $.identifier),
      "::",
      field("name", $.identifier),
      optional(seq(
        "<",
        field("generic_arg", $.generic_arg),
        repeat(seq(",", field("generic_arg", $.generic_arg))),
        optional(","),
        ">",
      )),
      "(",
      optional(seq(
        $._expr,
        repeat(seq(",", $._expr)),
        optional(","),
      )),
      ")",
    )),

    // A generic argument in turbofish position: either a type or a nat literal
    generic_arg: $ => choice(
      $.type_expr,
      $.number,
    ),

    // Primary expressions
    _primary_expr: $ => choice(
      $.number,
      $.boolean,
      $.unit_literal,
      $.graph_ref,
      $.struct_construction,
      $.map_literal,
      $.parenthesized_expr,
      $.qualified_variant,
      $.identifier,
    ),

    // Unit-annotated literal: 400 km, 9.80665 m/s^2
    // Uses dynamic precedence to prefer unit_literal over bare number
    // when followed by an identifier in expression context.
    unit_literal: $ => prec.dynamic(1, seq(
      field("value", $.number),
      field("unit", $.unit_expr),
    )),

    graph_ref: $ => seq(
      "@",
      optional(seq(field("module", $.identifier), "::")),
      field("name", $.identifier),
    ),

    // TransferResult { dv1, dv2: a + b, total_dv: dv1 + dv2 }
    struct_construction: $ => seq(
      field("type", $.identifier),
      optional(seq(
        "<",
        field("type_arg", $.type_expr),
        repeat(seq(",", field("type_arg", $.type_expr))),
        optional(","),
        ">",
      )),
      "{",
      optional(seq(
        $.field_init,
        repeat(seq(",", $.field_init)),
        optional(","),
      )),
      "}",
    ),

    field_init: $ => choice(
      // Explicit: field_name: expr
      seq(
        field("name", $.identifier),
        ":",
        field("value", $._expr),
      ),
      // Shorthand: field_name
      field("name", $.identifier),
    ),

    // { Maneuver::Departure: 2.46 km/s, ... }
    // { (Maneuver::Departure, Phase::Burn): 2.46 km/s, ... }
    map_literal: $ => seq(
      "{",
      optional(seq(
        choice($.map_entry, $.tuple_map_entry),
        repeat(seq(",", choice($.map_entry, $.tuple_map_entry))),
        optional(","),
      )),
      "}",
    ),

    map_entry: $ => seq(
      field("key", $.qualified_variant),
      ":",
      field("value", $._expr),
    ),

    tuple_map_entry: $ => seq(
      "(",
      field("key", $.qualified_variant),
      repeat1(seq(",", field("key", $.qualified_variant))),
      optional(","),
      ")",
      ":",
      field("value", $._expr),
    ),

    // A brace-delimited body used by if/for (single expression)
    brace_body: $ => seq(
      "{",
      field("value", $._expr),
      "}",
    ),

    parenthesized_expr: $ => seq(
      "(",
      $._expr,
      ")",
    ),

    // ---------------------------------------------------------------
    // Terminals
    // ---------------------------------------------------------------

    // Numeric literal with underscores and scientific notation
    number: $ => /[0-9][0-9_]*(\.[0-9][0-9_]*)?([eE][+-]?[0-9]+)?/,

    boolean: $ => choice("true", "false"),

    string_literal: $ => /"[^"]*"/,

    identifier: $ => /[a-zA-Z][a-zA-Z0-9_]*/,

    line_comment: $ => token(seq("//", /.*/)),
  },
});
