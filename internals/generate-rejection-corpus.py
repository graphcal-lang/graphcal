#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# ///
"""Generate high-entropy Graphcal programs that `graphcal check` must reject.

Each template seeds exactly one semantic (or syntactic) violation into an
otherwise well-formed program, and every template is instantiated many times
with randomized names, dimensions, units, magnitudes, filler declarations and
(for expression-level violations) surrounding syntactic context. A file that
survives `graphcal check` is therefore either a compiler hole or a template
whose seeded violation is legal after all -- both are worth reading.

Usage:

    internals/generate-rejection-corpus.py --count 800 --seed 1 --out /tmp/cases
    for f in /tmp/cases/*.gcl; do
        graphcal check "$f" >/dev/null 2>&1 && echo "ACCEPTED: $f"
    done

`MANIFEST.tsv` in the output directory records `file, template, reason` so an
accepted file can be traced back to the rule it was meant to violate.
"""

from __future__ import annotations

import argparse
import os
import random
import shutil
from collections.abc import Callable, Sequence

#: One generated case: the program text and why it must be rejected.
Case = tuple[str, str]

# ---------------------------------------------------------------------------
# dimension / unit vocabulary (prelude only)
# ---------------------------------------------------------------------------

# name -> (canonical signature, unit spellings)
DIMS = {
    "Length": ("L", ["m", "km", "cm", "mm"]),
    "Time": ("T", ["s", "min", "h"]),
    "Mass": ("M", ["kg", "g"]),
    "ElectricCurrent": ("I", ["A"]),
    "Amount": ("N", ["mol"]),
    "LuminousIntensity": ("J", ["cd"]),
    "Angle": ("A", ["rad", "deg"]),
    "Force": ("MLT-2", ["N", "kN"]),
    "Energy": ("ML2T-2", ["J", "kJ"]),
    "Power": ("ML2T-3", ["W", "kW"]),
    "Pressure": ("ML-1T-2", ["Pa", "kPa", "MPa"]),
    "Frequency": ("T-1", ["Hz", "1/s"]),
    "Velocity": ("LT-1", ["m/s", "km/s", "km/h"]),
    "Acceleration": ("LT-2", ["m/s^2", "km/s^2"]),
    "Area": ("L2", ["m^2", "km^2", "cm^2"]),
    "Volume": ("L3", ["m^3", "cm^3"]),
    "Dimensionless": ("1", [None]),
}
DIM_NAMES = [d for d in DIMS if d != "Dimensionless"]
ALL_DIM_NAMES = list(DIMS)


class Ctx:
    """Per-file naming/entropy helper."""

    def __init__(self, rng: random.Random, stem: str) -> None:
        self.r = rng
        self.stem = stem
        self.n = 0

    def low(self, hint: str = "v") -> str:
        self.n += 1
        return "%s_%s%d" % (hint, self.r.choice("abcdefghijkmnpqrstuvwxyz"), self.n)

    def up(self, hint: str = "T") -> str:
        self.n += 1
        return "%s%s%d" % (hint, self.r.choice("ABCDEFGHJKLMNPQRSTUVWXYZ"), self.n)

    def dim(self, exclude: Sequence[str] = ()) -> str:
        excl = {DIMS[d][0] for d in exclude}
        pool = [d for d in DIM_NAMES if DIMS[d][0] not in excl]
        return self.r.choice(pool)

    def dim2(self) -> tuple[str, str]:
        a = self.dim()
        b = self.dim(exclude=(a,))
        return a, b

    def unit(self, dim: str) -> str | None:
        return self.r.choice(DIMS[dim][1])

    def num(self, lo: float = 0.5, hi: float = 500.0) -> str:
        return "%.*f" % (self.r.randint(1, 4), self.r.uniform(lo, hi))

    def lit(self, dim: str) -> str:
        u = self.unit(dim)
        return self.num() if u is None else "%s %s" % (self.num(), u)

    def comment(self) -> str:
        if self.r.random() < 0.35:
            return "// %s\n" % self.r.choice(
                [
                    "generated case",
                    "entropy filler",
                    "seeded violation below",
                    "check must reject this file",
                    "do not format",
                ]
            )
        return ""


# ---------------------------------------------------------------------------
# valid filler declarations (pure entropy, must never introduce an error)
# ---------------------------------------------------------------------------


def filler(c: Ctx, count: int | None = None) -> str:
    if count is None:
        count = c.r.randint(0, 3)
    out = []
    for _ in range(count):
        kind = c.r.randrange(6)
        if kind == 0:
            d = c.dim()
            out.append("param %s: %s = %s;" % (c.low("p"), d, c.lit(d)))
        elif kind == 1:
            d = c.dim()
            out.append("const node %s: %s = %s;" % (c.low("k"), d, c.lit(d)))
        elif kind == 2:
            out.append("node %s: Int = %d;" % (c.low("i"), c.r.randint(-50, 50)))
        elif kind == 3:
            out.append("assert %s = %s < %s;" % (c.low("a"), c.num(1, 5), c.num(10, 50)))
        elif kind == 4:
            ix = c.up("Ix")
            out.append("index %s = { %s, %s };" % (ix, c.up("La"), c.up("Lb")))
        else:
            t = c.up("Rec")
            out.append("type %s { %s(%s: %s) }" % (t, t, c.low("f"), c.dim()))
    return "\n".join(out) + ("\n" if out else "")


def assemble(c: Ctx, core: str) -> str:
    """Sprinkle filler around the seeded-violation core."""
    parts = [filler(c), c.comment(), core, filler(c)]
    return "".join(p if p.endswith("\n") else p + "\n" for p in parts if p.strip())


# ---------------------------------------------------------------------------
# wrapper: put a wrongly-dimensioned expression in a typed position
# ---------------------------------------------------------------------------


def wrap_bad_expr(c: Ctx, target_dim: str, bad_expr: str, prelude: str = "") -> str:
    """Bind `bad_expr` (whose dimension is NOT target_dim) in a typed context."""
    style = c.r.randrange(5)
    n = c.low("bad")
    if style == 0:
        core = "node %s: %s = %s;" % (n, target_dim, bad_expr)
    elif style == 1:
        core = "node %s: %s = (%s);" % (n, target_dim, bad_expr)
    elif style == 2:
        f = c.low("flag")
        core = "param %s: Bool = %s;\nnode %s: %s = if @%s { %s } else { %s };" % (
            f,
            c.r.choice(["true", "false"]),
            n,
            target_dim,
            f,
            bad_expr,
            c.lit(target_dim),
        )
    elif style == 3:
        ix = c.up("Ax")
        v = c.low("it")
        core = "index %s = { %s, %s };\nnode %s: %s[%s] = for %s: %s { %s };" % (
            ix,
            c.up("E"),
            c.up("E"),
            n,
            target_dim,
            ix,
            v,
            ix,
            bad_expr,
        )
    else:
        t = c.up("Box")
        fld = c.low("fld")
        core = "type %s { %s(%s: %s) }\nnode %s: %s = %s(%s: %s);" % (
            t,
            t,
            fld,
            target_dim,
            n,
            t,
            t,
            fld,
            bad_expr,
        )
    return prelude + core


# ---------------------------------------------------------------------------
# templates: each returns (source, reason)
# ---------------------------------------------------------------------------

TEMPLATES: list[Callable[[Ctx], Case]] = []


def template(fn: Callable[[Ctx], Case]) -> Callable[[Ctx], Case]:
    """Register one violation template in declaration order."""
    TEMPLATES.append(fn)
    return fn


# --- dimension algebra -----------------------------------------------------


@template
def t_add_dim_mismatch(c: Ctx) -> Case:
    d1, d2 = c.dim2()
    a, b = c.low("a"), c.low("b")
    op = c.r.choice(["+", "-"])
    pre = "param %s: %s = %s;\nparam %s: %s = %s;\n" % (a, d1, c.lit(d1), b, d2, c.lit(d2))
    core = wrap_bad_expr(c, d1, "@%s %s @%s" % (a, op, b), pre)
    return core, "%s %s %s is a dimension mismatch" % (d1, op, d2)


@template
def t_annotation_mismatch(c: Ctx) -> Case:
    d1, d2 = c.dim2()
    return (
        wrap_bad_expr(c, d1, c.lit(d2)),
        "literal of dimension %s bound to a %s declaration" % (d2, d1),
    )


@template
def t_cmp_dim_mismatch(c: Ctx) -> Case:
    d1, d2 = c.dim2()
    a, b = c.low("a"), c.low("b")
    op = c.r.choice(["<", ">", "<=", ">=", "==", "!="])
    core = "param %s: %s = %s;\nparam %s: %s = %s;\nassert %s = @%s %s @%s;" % (
        a, d1, c.lit(d1), b, d2, c.lit(d2), c.low("as"), a, op, b,
    )
    return core, "comparison between %s and %s" % (d1, d2)


@template
def t_if_branch_mismatch(c: Ctx) -> Case:
    d1, d2 = c.dim2()
    f = c.low("flag")
    core = "param %s: Bool = true;\nnode %s: %s = if @%s { %s } else { %s };" % (
        f, c.low("bad"), d1, f, c.lit(d1), c.lit(d2),
    )
    return core, "if/else branches have dimensions %s and %s" % (d1, d2)


@template
def t_convert_wrong_dim(c: Ctx) -> Case:
    d1, d2 = c.dim2()
    if DIMS[d2][1][0] is None:
        d2 = "Length"
    a = c.low("a")
    core = "param %s: %s = %s;\nnode %s: %s = @%s -> %s;" % (
        a, d1, c.lit(d1), c.low("bad"), d1, a, c.unit(d2),
    )
    return core, "conversion of %s to a %s unit" % (d1, d2)


@template
def t_convert_chain(c: Ctx) -> Case:
    d = c.dim()
    us = DIMS[d][1]
    if len(us) < 2 or us[0] is None:
        d, us = "Length", DIMS["Length"][1]
    u1, u2 = c.r.sample(us, 2)
    a = c.low("a")
    core = "param %s: %s = %s;\nnode %s: %s = (@%s -> %s) -> %s;" % (
        a, d, c.lit(d), c.low("bad"), d, a, u1, u2,
    )
    return core, "chained unit conversion"


@template
def t_convert_in_arith(c: Ctx) -> Case:
    d = c.dim()
    if DIMS[d][1][0] is None:
        d = "Length"
    a = c.low("a")
    core = "param %s: %s = %s;\nnode %s: %s = (@%s -> %s) * %s;" % (
        a, d, c.lit(d), c.low("bad"), d, a, c.unit(d), c.num(1, 4),
    )
    return core, "conversion in a non-display position"


@template
def t_derived_dim_mismatch(c: Ctx) -> Case:
    dn = c.up("Dm")
    body = c.r.choice(["Length / Time^3", "Mass * Length^2 / Time^4", "Length^3 / Time^2"])
    d2 = c.r.choice(["Velocity", "Force", "Energy", "Length"])
    core = "dim %s = %s;\nnode %s: %s = %s;" % (dn, body, c.low("bad"), dn, c.lit(d2))
    return core, "value of %s bound to derived dim %s = %s" % (d2, dn, body)


@template
def t_unit_decl_dim_mismatch(c: Ctx) -> Case:
    d1, d2 = c.dim2()
    if DIMS[d2][1][0] is None:
        d2 = "Length"
    core = "const unit %s: %s = %s %s;" % (c.low("u"), d1, c.num(1, 9), c.unit(d2))
    return core, "unit declared as %s but defined with a %s unit" % (d1, d2)


@template
def t_unit_bad_scale(c: Ctx) -> Case:
    d = c.dim()
    if DIMS[d][1][0] is None:
        d = "Length"
    scale = c.r.choice(["0.0", "-" + c.num(1, 9), "-0.5"])
    core = "const unit %s: %s = %s %s;" % (c.low("u"), d, scale, c.unit(d))
    return core, "non-positive unit scale %s" % scale


@template
def t_affine_temperature_unit(c: Ctx) -> Case:
    core = "const unit %s: Temperature = %s K;" % (c.low("degx"), c.num(1, 3))
    return core, "user unit on bare Temperature (affine scale)"


@template
def t_dim_zero_exponent(c: Ctx) -> Case:
    dn = c.up("Dm")
    core = "dim %s = %s^0;\nnode %s: %s = %s;" % (
        dn, c.r.choice(["Length", "Time", "Mass"]), c.low("bad"), dn, c.num(),
    )
    return core, "zero exponent in a dimension expression"


@template
def t_unit_exponent_zero(c: Ctx) -> Case:
    a = c.low("a")
    core = "param %s: Dimensionless = %s;\nnode %s: Dimensionless = @%s -> %s^0;" % (
        a, c.num(), c.low("bad"), a, c.r.choice(["m", "s", "kg"]),
    )
    return core, "zero exponent in a conversion target"


@template
def t_power_dim_result(c: Ctx) -> Case:
    a = c.low("a")
    core = "param %s: Length = %s;\nnode %s: %s = @%s ^ 2;" % (
        a, c.lit("Length"), c.low("bad"), c.r.choice(["Length", "Volume"]), a,
    )
    return core, "Length^2 is Area, not the annotated dimension"


@template
def t_float_power_exponent(c: Ctx) -> Case:
    a = c.low("a")
    d = c.r.choice(["Length", "Mass", "Time"])
    core = "param %s: %s = %s;\nnode %s: %s = @%s ^ %s;" % (
        a, d, c.lit(d), c.low("bad"), d, a, c.r.choice(["0.5", "2.0", "0.25", "1.5"]),
    )
    return core, "decimal exponent on a dimensioned base"


@template
def t_runtime_exponent(c: Ctx) -> Case:
    a, e = c.low("a"), c.low("e")
    core = (
        "param %s: Length = %s;\nparam %s: Dimensionless = %s;\nnode %s: Area = @%s ^ @%s;"
        % (a, c.lit("Length"), e, c.num(1, 4), c.low("bad"), a, e)
    )
    return core, "runtime exponent on a dimensioned base"


@template
def t_sqrt_result_mismatch(c: Ctx) -> Case:
    d = c.r.choice(["Area", "Volume", "Energy"])
    a = c.low("a")
    core = "param %s: %s = %s;\nnode %s: %s = sqrt(@%s);" % (
        a, d, c.lit(d), c.low("bad"), d, a,
    )
    return core, "sqrt halves the dimension of %s" % d


# --- Int / Bool ------------------------------------------------------------


@template
def t_int_float_mix(c: Ctx) -> Case:
    i = c.low("i")
    core = "param %s: Int = %d;\nnode %s: Int = @%s %s %s;" % (
        i, c.r.randint(1, 40), c.low("bad"), i, c.r.choice(["+", "-", "*"]), c.num(1, 9),
    )
    return core, "Int mixed with a Dimensionless quantity"


@template
def t_int_annotation_float(c: Ctx) -> Case:
    if c.r.random() < 0.5:
        core = "node %s: Int = %s;" % (c.low("bad"), c.num())
        return core, "float literal bound to Int"
    core = "node %s: Dimensionless = %d;" % (c.low("bad"), c.r.randint(1, 99))
    return core, "integer literal bound to Dimensionless"


@template
def t_bool_from_number(c: Ctx) -> Case:
    core = "node %s: Bool = %s;" % (
        c.low("bad"), c.r.choice([str(c.r.randint(0, 9)), c.num()]),
    )
    return core, "numeric literal bound to Bool"


@template
def t_not_on_non_bool(c: Ctx) -> Case:
    x = c.low("x")
    if c.r.random() < 0.5:
        core = "param %s: Int = %d;\nnode %s: Bool = !@%s;" % (
            x, c.r.randint(1, 9), c.low("bad"), x,
        )
        return core, "logical not applied to Int"
    d = c.dim()
    core = "param %s: %s = %s;\nnode %s: Bool = !@%s;" % (x, d, c.lit(d), c.low("bad"), x)
    return core, "logical not applied to %s" % d


@template
def t_mod_on_float(c: Ctx) -> Case:
    a, b = c.low("a"), c.low("b")
    core = (
        "param %s: Dimensionless = %s;\nparam %s: Dimensionless = %s;\n"
        "node %s: Dimensionless = @%s %% @%s;" % (a, c.num(), b, c.num(), c.low("bad"), a, b)
    )
    return core, "%% requires Int operands"


@template
def t_logical_on_quantity(c: Ctx) -> Case:
    a, b = c.low("a"), c.low("b")
    d = c.r.choice(["Dimensionless", "Length"])
    core = "param %s: %s = %s;\nparam %s: %s = %s;\nnode %s: Bool = @%s %s @%s;" % (
        a, d, c.lit(d), b, d, c.lit(d), c.low("bad"), a, c.r.choice(["&&", "||"]), b,
    )
    return core, "logical operator on %s operands" % d


@template
def t_int_negative_power(c: Ctx) -> Case:
    i = c.low("i")
    core = "param %s: Int = %d;\nnode %s: Int = @%s ^ -%d;" % (
        i, c.r.randint(2, 9), c.low("bad"), i, c.r.randint(1, 5),
    )
    return core, "negative Int exponent"


@template
def t_to_int_non_integer(c: Ctx) -> Case:
    core = "const node %s: Int = to_int(%s);" % (
        c.low("bad"), c.r.choice(["3.7", "0.5", "-2.25", "1.0001"]),
    )
    return core, "to_int of a non-integral constant"


@template
def t_to_float_on_float(c: Ctx) -> Case:
    core = "node %s: Dimensionless = to_float(%s);" % (c.low("bad"), c.num())
    return core, "to_float applied to a Dimensionless value"


# --- built-in functions ----------------------------------------------------


@template
def t_builtin_arity(c: Ctx) -> Case:
    fn, args = c.r.choice(
        [
            ("sqrt", ["%s, %s" % (c.num(), c.num())]),
            ("clamp", ["%s, %s" % (c.num(), c.num())]),
            ("hypot", ["%s" % c.num()]),
            ("log", ["%s" % c.num()]),
            ("atan2", ["%s" % c.num()]),
            ("abs", [""]),
            ("least", ["%s" % c.num()]),
            ("greatest", ["%s, %s, %s" % (c.num(), c.num(), c.num())]),
            ("complex", ["%s" % c.num()]),
        ]
    )
    core = "node %s: Dimensionless = %s(%s);" % (c.low("bad"), fn, args[0])
    return core, "wrong arity for %s" % fn


@template
def t_trig_wrong_dim(c: Ctx) -> Case:
    fn = c.r.choice(["sin", "cos", "tan"])
    arg = c.r.choice([c.num(), c.lit("Length"), c.lit("Time")])
    core = "node %s: Dimensionless = %s(%s);" % (c.low("bad"), fn, arg)
    return core, "%s requires an Angle argument" % fn


@template
def t_inverse_trig_wrong_dim(c: Ctx) -> Case:
    fn = c.r.choice(["asin", "acos", "atan"])
    core = "node %s: Angle = %s(%s);" % (c.low("bad"), fn, c.lit(c.dim()))
    return core, "%s requires a Dimensionless argument" % fn


@template
def t_rounding_dimensioned(c: Ctx) -> Case:
    fn = c.r.choice(["round", "trunc", "floor", "ceil"])
    d = c.dim()
    core = "node %s: Dimensionless = %s(%s);" % (c.low("bad"), fn, c.lit(d))
    return core, "%s rejects the dimensioned argument %s" % (fn, d)


@template
def t_transcendental_dimensioned(c: Ctx) -> Case:
    fn = c.r.choice(["exp", "ln", "log2", "log10", "expm1", "log1p", "sinh", "tanh"])
    d = c.dim()
    core = "node %s: Dimensionless = %s(%s);" % (c.low("bad"), fn, c.lit(d))
    return core, "%s requires Dimensionless, got %s" % (fn, d)


@template
def t_unknown_function(c: Ctx) -> Case:
    core = "node %s: Dimensionless = %s(%s);" % (
        c.low("bad"), c.low("fn").replace("_", ""), c.num(),
    )
    return core, "call to an undefined function"


@template
def t_named_args_builtin(c: Ctx) -> Case:
    fn = c.r.choice(["sqrt", "abs", "ln", "sign"])
    core = "node %s: Dimensionless = %s(%s: %s);" % (c.low("bad"), fn, c.low("arg"), c.num())
    return core, "named argument passed to built-in %s" % fn


@template
def t_two_arg_dim_mismatch(c: Ctx) -> Case:
    fn = c.r.choice(["least", "greatest", "hypot", "atan2", "complex"])
    d1, d2 = c.dim2()
    ann = "Angle" if fn == "atan2" else ("Complex<%s>" % d1 if fn == "complex" else d1)
    core = "node %s: %s = %s(%s, %s);" % (c.low("bad"), ann, fn, c.lit(d1), c.lit(d2))
    return core, "%s over mismatched dimensions %s / %s" % (fn, d1, d2)


@template
def t_clamp_dim_mismatch(c: Ctx) -> Case:
    d1, d2 = c.dim2()
    core = "node %s: %s = clamp(%s, %s, %s);" % (
        c.low("bad"), d1, c.lit(d1), c.lit(d2), c.lit(d1),
    )
    return core, "clamp bounds of dimension %s against %s" % (d2, d1)


# --- algebraic data types --------------------------------------------------


def _adt_decl(c: Ctx, nfields: int = 2) -> tuple[str, list[tuple[str, str]], str]:
    t = c.up("St")
    flds = [(c.low("f"), c.dim()) for _ in range(nfields)]
    decl = "type %s {\n    %s(%s),\n}" % (
        t, t, ", ".join("%s: %s" % (n, d) for n, d in flds),
    )
    return t, flds, decl


@template
def t_adt_missing_field(c: Ctx) -> Case:
    t, flds, decl = _adt_decl(c, c.r.randint(2, 3))
    used = flds[:-1]
    core = "%s\nnode %s: %s = %s(%s);" % (
        decl, c.low("bad"), t, t, ", ".join("%s: %s" % (n, c.lit(d)) for n, d in used),
    )
    return core, "constructor call omits field %s" % flds[-1][0]


@template
def t_adt_unknown_field(c: Ctx) -> Case:
    t, flds, decl = _adt_decl(c, c.r.randint(1, 2))
    args = ["%s: %s" % (n, c.lit(d)) for n, d in flds]
    args.append("%s: %s" % (c.low("zz"), c.lit(c.dim())))
    core = "%s\nnode %s: %s = %s(%s);" % (decl, c.low("bad"), t, t, ", ".join(args))
    return core, "constructor call passes an unknown field"


@template
def t_adt_field_dim_mismatch(c: Ctx) -> Case:
    t, flds, decl = _adt_decl(c, c.r.randint(1, 3))
    i = c.r.randrange(len(flds))
    args = []
    for j, (n, d) in enumerate(flds):
        if j == i:
            other = c.dim(exclude=(d,))
            args.append("%s: %s" % (n, c.lit(other)))
        else:
            args.append("%s: %s" % (n, c.lit(d)))
    core = "%s\nnode %s: %s = %s(%s);" % (decl, c.low("bad"), t, t, ", ".join(args))
    return core, "constructor field %s got the wrong dimension" % flds[i][0]


@template
def t_adt_field_access_multi_ctor(c: Ctx) -> Case:
    t = c.up("Un")
    v1, v2 = c.up("Ca"), c.up("Cb")
    f = c.low("f")
    d = c.dim()
    val = c.low("val")
    core = (
        "type %s {\n    %s(%s: %s),\n    %s,\n}\n"
        "node %s: %s = %s(%s: %s);\n"
        "node %s: %s = @%s.%s;" % (t, v1, f, d, v2, val, t, v1, f, c.lit(d),
                                   c.low("bad"), d, val, f)
    )
    return core, "field access on a multi-constructor type"


@template
def t_adt_unknown_field_access(c: Ctx) -> Case:
    t, flds, decl = _adt_decl(c, 2)
    val = c.low("val")
    core = "%s\nnode %s: %s = %s(%s);\nnode %s: %s = @%s.%s;" % (
        decl, val, t, t, ", ".join("%s: %s" % (n, c.lit(d)) for n, d in flds),
        c.low("bad"), flds[0][1], val, c.low("nofield"),
    )
    return core, "access to a field the constructor does not declare"


@template
def t_adt_field_access_on_primitive(c: Ctx) -> Case:
    a = c.low("a")
    d = c.dim()
    core = "param %s: %s = %s;\nnode %s: %s = @%s.%s;" % (
        a, d, c.lit(d), c.low("bad"), d, a, c.low("fld"),
    )
    return core, "field access on a primitive quantity"


@template
def t_match_non_exhaustive(c: Ctx) -> Case:
    t = c.up("Un")
    vs = [c.up("Vv") for _ in range(c.r.randint(2, 4))]
    d = c.dim()
    val = c.low("val")
    arms = "\n".join("    %s => %s," % (v, c.lit(d)) for v in vs[:-1])
    core = "type %s {\n%s\n}\nnode %s: %s = %s;\nnode %s: %s = match @%s {\n%s\n};" % (
        t, ",\n".join("    " + v for v in vs), val, t, vs[0], c.low("bad"), d, val, arms,
    )
    return core, "match omits constructor %s" % vs[-1]


@template
def t_match_arm_dim_mismatch(c: Ctx) -> Case:
    t = c.up("Un")
    v1, v2 = c.up("Vv"), c.up("Vv")
    d1, d2 = c.dim2()
    val = c.low("val")
    core = (
        "type %s {\n    %s,\n    %s,\n}\nnode %s: %s = %s;\n"
        "node %s: %s = match @%s {\n    %s => %s,\n    %s => %s,\n};"
        % (t, v1, v2, val, t, v1, c.low("bad"), d1, val, v1, c.lit(d1), v2, c.lit(d2))
    )
    return core, "match arms produce %s and %s" % (d1, d2)


@template
def t_match_duplicate_arm(c: Ctx) -> Case:
    t = c.up("Un")
    v1, v2 = c.up("Vv"), c.up("Vv")
    d = c.dim()
    val = c.low("val")
    core = (
        "type %s {\n    %s,\n    %s,\n}\nnode %s: %s = %s;\n"
        "node %s: %s = match @%s {\n    %s => %s,\n    %s => %s,\n    %s => %s,\n};"
        % (t, v1, v2, val, t, v1, c.low("bad"), d, val, v1, c.lit(d), v1, c.lit(d),
           v2, c.lit(d))
    )
    return core, "duplicate match arm for %s" % v1


@template
def t_match_unknown_variant(c: Ctx) -> Case:
    t = c.up("Un")
    v1, v2 = c.up("Vv"), c.up("Vv")
    d = c.dim()
    val = c.low("val")
    core = (
        "type %s {\n    %s,\n    %s,\n}\nnode %s: %s = %s;\n"
        "node %s: %s = match @%s {\n    %s => %s,\n    %s => %s,\n    %s => %s,\n};"
        % (t, v1, v2, val, t, v1, c.low("bad"), d, val, v1, c.lit(d), v2, c.lit(d),
           c.up("Ghost"), c.lit(d))
    )
    return core, "match arm names an undeclared constructor"


@template
def t_match_on_quantity(c: Ctx) -> Case:
    a = c.low("a")
    d = c.dim()
    core = "param %s: %s = %s;\nnode %s: %s = match @%s {\n    %s => %s,\n};" % (
        a, d, c.lit(d), c.low("bad"), d, a, c.up("Any"), c.lit(d),
    )
    return core, "match on a quantity value"


@template
def t_generic_arity(c: Ctx) -> Case:
    t = c.up("Gv")
    # The phantom `Type` parameter needs a marker type whose constructor shares
    # its name, so the only missing argument is the one under test.
    mk = c.up("Fr")
    core = (
        "type %s { %s }\ntype %s<D: Dim, F: Type> {\n    %s(x: D, y: D),\n}\n"
        "node %s: %s<Length> = %s<Length>(x: %s, y: %s);"
        % (mk, mk, t, t, c.low("bad"), t, t, c.lit("Length"), c.lit("Length"))
    )
    return core, "generic type applied with too few arguments"


@template
def t_generic_sort_mismatch(c: Ctx) -> Case:
    t = c.up("Gv")
    core = (
        "type %s<D: Dim> {\n    %s(x: D),\n}\nnode %s: %s<%d> = %s<%d>(x: %s);"
        % (t, t, c.low("bad"), t, c.r.randint(2, 5), t, c.r.randint(2, 5), c.num())
    )
    return core, "Nat argument supplied for a Dim parameter"


@template
def t_indexed_type_arg(c: Ctx) -> Case:
    t = c.up("Hd")
    ix = c.up("Ax")
    d = c.dim()
    core = (
        "index %s = { %s, %s };\ntype %s<T: Type> {\n    %s(v: T),\n}\n"
        "node %s: %s<%s[%s]> = %s<%s[%s]>(v: %s);"
        % (ix, c.up("E"), c.up("E"), t, t, c.low("bad"), t, d, ix, t, d, ix,
           "{ %s.%s: %s, %s.%s: %s }" % (ix, "X", c.lit(d), ix, "Y", c.lit(d)))
    )
    return core, "indexed DeclType passed as a Type generic argument"


@template
def t_unit_ctor_with_payload(c: Ctx) -> Case:
    t = c.up("Mk")
    core = "type %s { %s }\nnode %s: %s = %s(%s: %s);" % (
        t, t, c.low("bad"), t, t, c.low("f"), c.num(),
    )
    return core, "payload passed to a unit constructor"


# --- indexes ---------------------------------------------------------------


def _index_decl(c: Ctx, n: int = 3) -> tuple[str, list[str], str]:
    ix = c.up("Ax")
    labels = [c.up("Lb") for _ in range(n)]
    return ix, labels, "index %s = { %s };" % (ix, ", ".join(labels))


@template
def t_map_missing_label(c: Ctx) -> Case:
    ix, labels, decl = _index_decl(c, c.r.randint(2, 4))
    d = c.dim()
    entries = ",\n".join("    %s.%s: %s" % (ix, l, c.lit(d)) for l in labels[:-1])
    core = "%s\nparam %s: %s[%s] = {\n%s,\n};" % (decl, c.low("bad"), d, ix, entries)
    return core, "map literal omits label %s" % labels[-1]


@template
def t_map_extra_label(c: Ctx) -> Case:
    ix, labels, decl = _index_decl(c, c.r.randint(2, 3))
    d = c.dim()
    entries = [("    %s.%s: %s" % (ix, l, c.lit(d))) for l in labels]
    entries.append("    %s.%s: %s" % (ix, c.up("Ghost"), c.lit(d)))
    core = "%s\nparam %s: %s[%s] = {\n%s,\n};" % (decl, c.low("bad"), d, ix, ",\n".join(entries))
    return core, "map literal names a label the index does not declare"


@template
def t_map_wrong_axis(c: Ctx) -> Case:
    ix, labels, decl = _index_decl(c, 2)
    jx, jlabels, jdecl = _index_decl(c, 2)
    d = c.dim()
    entries = ",\n".join("    %s.%s: %s" % (jx, l, c.lit(d)) for l in jlabels)
    core = "%s\n%s\nparam %s: %s[%s] = {\n%s,\n};" % (
        decl, jdecl, c.low("bad"), d, ix, entries,
    )
    return core, "map literal keyed by %s bound to a %s axis" % (jx, ix)


@template
def t_map_bare_label(c: Ctx) -> Case:
    ix, labels, decl = _index_decl(c, 2)
    d = c.dim()
    entries = ",\n".join("    %s: %s" % (l, c.lit(d)) for l in labels)
    core = "%s\nparam %s: %s[%s] = {\n%s,\n};" % (decl, c.low("bad"), d, ix, entries)
    return core, "unqualified labels in a map literal"


@template
def t_map_entry_dim_mismatch(c: Ctx) -> Case:
    ix, labels, decl = _index_decl(c, c.r.randint(2, 3))
    d1, d2 = c.dim2()
    bad_at = c.r.randrange(len(labels))
    entries = ",\n".join(
        "    %s.%s: %s" % (ix, l, c.lit(d2 if i == bad_at else d1))
        for i, l in enumerate(labels)
    )
    core = "%s\nparam %s: %s[%s] = {\n%s,\n};" % (decl, c.low("bad"), d1, ix, entries)
    return core, "map entry has dimension %s in a %s axis" % (d2, d1)


@template
def t_index_wrong_axis_access(c: Ctx) -> Case:
    ix, labels, decl = _index_decl(c, 2)
    jx, jlabels, jdecl = _index_decl(c, 2)
    d = c.dim()
    val = c.low("val")
    entries = ", ".join("%s.%s: %s" % (ix, l, c.lit(d)) for l in labels)
    core = "%s\n%s\nparam %s: %s[%s] = { %s };\nnode %s: %s = @%s[%s.%s];" % (
        decl, jdecl, val, d, ix, entries, c.low("bad"), d, val, jx, jlabels[0],
    )
    return core, "%s key used to index a %s axis" % (jx, ix)


@template
def t_index_unindexed(c: Ctx) -> Case:
    ix, labels, decl = _index_decl(c, 2)
    d = c.dim()
    a = c.low("a")
    core = "%s\nparam %s: %s = %s;\nnode %s: %s = @%s[%s.%s];" % (
        decl, a, d, c.lit(d), c.low("bad"), d, a, ix, labels[0],
    )
    return core, "index access on an unindexed value"


@template
def t_index_arity(c: Ctx) -> Case:
    ix, labels, decl = _index_decl(c, 2)
    jx, jlabels, jdecl = _index_decl(c, 2)
    d = c.dim()
    val = c.low("val")
    entries = ",\n".join(
        "    (%s.%s, %s.%s): %s" % (ix, a, jx, b, c.lit(d)) for a in labels for b in jlabels
    )
    core = "%s\n%s\nparam %s: %s[%s, %s] = {\n%s,\n};\nnode %s: %s = @%s[%s.%s];" % (
        decl, jdecl, val, d, ix, jx, entries, c.low("bad"), d, val, ix, labels[0],
    )
    return core, "two-axis value accessed with one key"


@template
def t_indexed_comparison(c: Ctx) -> Case:
    ix, labels, decl = _index_decl(c, 2)
    d = c.dim()
    val = c.low("val")
    entries = ", ".join("%s.%s: %s" % (ix, l, c.lit(d)) for l in labels)
    core = "%s\nnode %s: %s[%s] = { %s };\nnode %s: Bool[%s] = @%s %s %s;" % (
        decl, val, d, ix, entries, c.low("bad"), ix, val,
        c.r.choice(["<", ">", "<=", ">=", "=="]), c.lit(d),
    )
    return core, "comparison operator applied to an indexed operand"


@template
def t_agg_multi_axis(c: Ctx) -> Case:
    ix, labels, decl = _index_decl(c, 2)
    jx, jlabels, jdecl = _index_decl(c, 2)
    d = c.dim()
    val = c.low("val")
    fn = c.r.choice(["sum", "maximum", "minimum", "mean", "rss"])
    core = (
        "%s\n%s\nnode %s: %s[%s, %s] = for %s: %s, %s: %s { %s };\nnode %s: %s = %s(@%s);"
        % (decl, jdecl, val, d, ix, jx, "ii", ix, "jj", jx, c.lit(d), c.low("bad"), d,
           fn, val)
    )
    return core, "%s applied to a two-axis value" % fn


@template
def t_agg_unindexed(c: Ctx) -> Case:
    d = c.dim()
    a = c.low("a")
    fn = c.r.choice(["sum", "maximum", "minimum", "mean", "count", "argmax"])
    ann = "Int" if fn == "count" else d
    core = "param %s: %s = %s;\nnode %s: %s = %s(@%s);" % (
        a, d, c.lit(d), c.low("bad"), ann, fn, a,
    )
    return core, "%s applied to an unindexed value" % fn


@template
def t_axis_order_mismatch(c: Ctx) -> Case:
    ix, ilabels, idecl = _index_decl(c, 2)
    jx, jlabels, jdecl = _index_decl(c, 2)
    d = c.dim()
    core = "%s\n%s\nnode %s: %s[%s, %s] = for %s: %s, %s: %s { %s };" % (
        idecl, jdecl, c.low("bad"), d, jx, ix, "aa", ix, "bb", jx, c.lit(d),
    )
    return core, "comprehension axis order does not match the annotation"


@template
def t_runtime_int_index(c: Ctx) -> Case:
    n = c.r.randint(2, 6)
    d = c.dim()
    val, idx = c.low("val"), c.low("idx")
    core = (
        "param %s: %s[Fin(%d)] = for %s: Fin(%d) { %s };\nparam %s: Int = %d;\n"
        "node %s: %s = @%s[@%s];"
        % (val, d, n, "ff", n, c.lit(d), idx, c.r.randint(0, n - 1), c.low("bad"), d,
           val, idx)
    )
    return core, "runtime Int used directly as a Fin index"


@template
def t_key_extraction_mismatch(c: Ctx) -> Case:
    d = c.dim()
    if c.r.random() < 0.5:
        ix, labels, decl = _index_decl(c, 2)
        val = c.low("val")
        entries = ", ".join("%s.%s: %s" % (ix, l, c.lit(d)) for l in labels)
        core = (
            "%s\nnode %s: %s[%s] = { %s };\nnode %s: Int = to_int(argmax(@%s));"
            % (decl, val, d, ix, entries, c.low("bad"), val)
        )
        return core, "to_int applied to a named-axis key"
    n = c.r.randint(2, 5)
    val = c.low("val")
    core = (
        "node %s: %s[Fin(%d)] = for %s: Fin(%d) { %s };\nnode %s: %s = coord(argmax(@%s));"
        % (val, d, n, "gg", n, c.lit(d), c.low("bad"), d, val)
    )
    return core, "coord applied to a Fin key"


@template
def t_key_static_out_of_bounds(c: Ctx) -> Case:
    n = c.r.randint(2, 6)
    core = "node %s: Key<Fin(%d)> = key(Fin(%d), %d);" % (
        c.low("bad"), n, n, n + c.r.randint(0, 4),
    )
    return core, "static Fin key position out of range"


@template
def t_key_arithmetic(c: Ctx) -> Case:
    n = c.r.randint(3, 6)
    form = c.r.choice(["sub", "mul", "runtime"])
    if form == "sub":
        core = "node %s: Key<Fin(%d)> = key(Fin(%d), 1) - 1;" % (c.low("bad"), n, n)
        return core, "key subtraction"
    if form == "mul":
        core = "node %s: Key<Fin(%d)> = key(Fin(%d), 1) * 2;" % (c.low("bad"), n, n)
        return core, "key multiplication"
    k = c.low("k")
    core = "param %s: Int = 1;\nnode %s: Key<Fin(%d)> = key(Fin(%d), 0) + @%s;" % (
        k, c.low("bad"), n + 1, n, k,
    )
    return core, "runtime addend in key arithmetic"


@template
def t_key_from_int(c: Ctx) -> Case:
    n = c.r.randint(2, 6)
    core = "node %s: Key<Fin(%d)> = %d;" % (c.low("bad"), n, c.r.randrange(n))
    return core, "integer literal bound to a Key type"


@template
def t_key_cross_axis_eq(c: Ctx) -> Case:
    ix, ilabels, idecl = _index_decl(c, 2)
    jx, jlabels, jdecl = _index_decl(c, 2)
    core = "%s\n%s\nnode %s: Bool = %s.%s == %s.%s;" % (
        idecl, jdecl, c.low("bad"), ix, ilabels[0], jx, jlabels[0],
    )
    return core, "equality between keys of different axes"


@template
def t_scan_multi_axis(c: Ctx) -> Case:
    ix, ilabels, idecl = _index_decl(c, 2)
    jx, jlabels, jdecl = _index_decl(c, 2)
    d = "Dimensionless"
    src = c.low("src")
    core = (
        "%s\n%s\nnode %s: %s[%s, %s] = for %s: %s, %s: %s { %s };\n"
        "node %s: %s[%s] = scan(@%s, %s, |%s, %s| %s + sum(%s));"
        % (idecl, jdecl, src, d, ix, jx, "pp", ix, "qq", jx, c.num(), c.low("bad"), d,
           ix, src, c.num(), "acc", "row", "acc", "row")
    )
    return core, "scan over a two-axis source"


@template
def t_scan_init_mismatch(c: Ctx) -> Case:
    ix, labels, decl = _index_decl(c, 3)
    d1, d2 = c.dim2()
    src = c.low("src")
    entries = ", ".join("%s.%s: %s" % (ix, l, c.lit(d1)) for l in labels)
    core = (
        "%s\nnode %s: %s[%s] = { %s };\nnode %s: %s[%s] = scan(@%s, %s, |acc, it| acc + it);"
        % (decl, src, d1, ix, entries, c.low("bad"), d1, ix, src, c.lit(d2))
    )
    return core, "scan initial accumulator has dimension %s, source %s" % (d2, d1)


@template
def t_table_row_mismatch(c: Ctx) -> Case:
    ix, ilabels, idecl = _index_decl(c, 2)
    jx, jlabels, jdecl = _index_decl(c, 3)
    d = c.dim()
    rows = []
    for i, rl in enumerate(ilabels):
        cells = [c.lit(d) for _ in jlabels]
        if i == 0:
            cells = cells[:-1]
        rows.append("    %s: %s;" % (rl, ", ".join(cells)))
    core = "%s\n%s\nparam %s: %s[%s, %s] = table[%s, %s] {\n    : %s;\n%s\n};" % (
        idecl, jdecl, c.low("bad"), d, ix, jx, ix, jx, ", ".join(jlabels), "\n".join(rows),
    )
    return core, "table row has too few cells"


@template
def t_table_wrong_label(c: Ctx) -> Case:
    ix, ilabels, idecl = _index_decl(c, 2)
    d = c.dim()
    rows = "\n".join("    %s: %s;" % (l, c.lit(d)) for l in ilabels[:-1])
    rows += "\n    %s: %s;" % (c.up("Ghost"), c.lit(d))
    core = "%s\nparam %s: %s[%s] = table[%s] {\n%s\n};" % (
        idecl, c.low("bad"), d, ix, ix, rows,
    )
    return core, "table row label is not a label of the axis"


@template
def t_range_index_invalid(c: Ctx) -> Case:
    ix = c.up("Cx")
    form = c.r.choice(["zero", "negative", "dim", "nonlanding", "points"])
    if form == "zero":
        decl = "index %s = range(0.0 s, %s s, step: 0.0 s);" % (ix, c.num(1, 9))
        why = "zero range step"
    elif form == "negative":
        decl = "index %s = range(0.0 s, %s s, step: -%s s);" % (ix, c.num(1, 9), c.num(0.1, 1))
        why = "step direction opposite to the range"
    elif form == "dim":
        decl = "index %s = range(0.0 s, %s s, step: %s m);" % (ix, c.num(1, 9), c.num(0.1, 1))
        why = "range step of a different dimension"
    elif form == "nonlanding":
        decl = "index %s = range(0.0 s, 1.0 s, step: 0.3 s);" % ix
        why = "step does not land on the endpoint"
    else:
        decl = "index %s = linspace(0.0 s, 1.0 s, points: 0);" % ix
        why = "linspace with zero points"
    d = "Time"
    core = "%s\nnode %s: %s[%s] = for %s: %s { coord(%s) };" % (
        decl, c.low("bad"), d, ix, "tt", ix, "tt",
    )
    return core, why


@template
def t_duplicate_index_label(c: Ctx) -> Case:
    ix = c.up("Ax")
    l = c.up("Lb")
    core = "index %s = { %s, %s, %s };" % (ix, l, c.up("Lb"), l)
    return core, "duplicate label in an index declaration"


@template
def t_fin_zero(c: Ctx) -> Case:
    d = c.dim()
    core = "node %s: %s[Fin(0)] = for %s: Fin(0) { %s };" % (
        c.low("bad"), d, "zz", c.lit(d),
    )
    return core, "Fin(0) is an empty axis"


@template
def t_fin_widen_violation(c: Ctx) -> Case:
    small = c.r.randint(2, 4)
    big = small + c.r.randint(1, 3)
    core = "node %s: Key<Fin(%d)> = key(Fin(%d), %d);" % (
        c.low("bad"), small, big, big - 1,
    )
    return core, "Key<Fin(%d)> assigned to Key<Fin(%d)>" % (big, small)


@template
def t_linalg_axis_mismatch(c: Ctx) -> Case:
    d1, d2 = "Length", "Dimensionless"
    form = c.r.choice(["matmul", "dot", "cross", "trace", "transpose", "norm"])
    n, m = c.r.randint(2, 4), c.r.randint(2, 4)
    if m == n:
        m = n + 1
    if form == "matmul":
        a, b = c.low("a"), c.low("b")
        core = (
            "param %s: %s[Fin(%d), Fin(%d)] = for i: Fin(%d), j: Fin(%d) { %s };\n"
            "param %s: %s[Fin(%d), Fin(%d)] = for i: Fin(%d), j: Fin(%d) { %s };\n"
            "node %s: %s[Fin(%d), Fin(%d)] = matmul(@%s, @%s);"
            % (a, d1, n, m, n, m, c.lit(d1), b, d2, n, m, n, m, c.num(),
               c.low("bad"), d1, n, m, a, b)
        )
        return core, "matmul contracted axes have different extents"
    if form == "dot":
        a, b = c.low("a"), c.low("b")
        core = (
            "param %s: %s[Fin(%d)] = for i: Fin(%d) { %s };\n"
            "param %s: %s[Fin(%d)] = for i: Fin(%d) { %s };\n"
            "node %s: %s = dot(@%s, @%s);"
            % (a, d1, n, n, c.lit(d1), b, d2, m, m, c.num(), c.low("bad"), d1, a, b)
        )
        return core, "dot over axes of different extents"
    if form == "cross":
        k = c.r.choice([2, 4, 5])
        a, b = c.low("a"), c.low("b")
        core = (
            "param %s: %s[Fin(%d)] = for i: Fin(%d) { %s };\n"
            "param %s: %s[Fin(%d)] = for i: Fin(%d) { %s };\n"
            "node %s: %s[Fin(%d)] = cross(@%s, @%s);"
            % (a, d1, k, k, c.lit(d1), b, d2, k, k, c.num(), c.low("bad"), d1, k, a, b)
        )
        return core, "cross product on a %d-element axis" % k
    if form == "trace":
        a = c.low("a")
        core = (
            "param %s: %s[Fin(%d), Fin(%d)] = for i: Fin(%d), j: Fin(%d) { %s };\n"
            "node %s: %s = trace(@%s);"
            % (a, d1, n, m, n, m, c.lit(d1), c.low("bad"), d1, a)
        )
        return core, "trace of a non-square matrix"
    if form == "transpose":
        a = c.low("a")
        core = (
            "param %s: %s[Fin(%d)] = for i: Fin(%d) { %s };\nnode %s: %s[Fin(%d)] = transpose(@%s);"
            % (a, d1, n, n, c.lit(d1), c.low("bad"), d1, n, a)
        )
        return core, "transpose of a rank-one value"
    a = c.low("a")
    core = (
        "param %s: %s[Fin(%d), Fin(%d)] = for i: Fin(%d), j: Fin(%d) { %s };\n"
        "node %s: %s = norm(@%s);" % (a, d1, n, m, n, m, c.lit(d1), c.low("bad"), d1, a)
    )
    return core, "norm of a rank-two value"


@template
def t_product_abstract_axis(c: Ctx) -> Case:
    g = c.low("dg")
    ax, vals, res = c.up("Ax"), c.low("vals"), c.low("res")
    fn = "product"
    core = (
        "dag %s {\n    pub(bind) index %s;\n    param %s: Length[%s];\n"
        "    pub node %s: Length = %s(@%s);\n}" % (g, ax, vals, ax, res, fn, vals)
    )
    return core, "%s over an abstract axis has no known cardinality" % fn


# --- declaration level -----------------------------------------------------


@template
def t_duplicate_decl(c: Ctx) -> Case:
    n = c.low("dup")
    d = c.dim()
    kind = c.r.choice(["param", "node"])
    core = "%s %s: %s = %s;\n%s %s: %s = %s;" % (
        kind, n, d, c.lit(d), kind, n, d, c.lit(d),
    )
    return core, "duplicate declaration of %s" % n


@template
def t_unknown_ref(c: Ctx) -> Case:
    d = c.dim()
    core = "node %s: %s = @%s + %s;" % (c.low("bad"), d, c.low("ghost"), c.lit(d))
    return core, "reference to an undeclared graph value"


@template
def t_cycle(c: Ctx) -> Case:
    a, b = c.low("a"), c.low("b")
    d = c.dim()
    if c.r.random() < 0.5:
        core = "node %s: %s = @%s;\nnode %s: %s = @%s;" % (a, d, b, b, d, a)
        return core, "dependency cycle between %s and %s" % (a, b)
    core = "node %s: %s = @%s + %s;" % (a, d, a, c.lit(d))
    return core, "self-referential node"


@template
def t_const_refs_param(c: Ctx) -> Case:
    p = c.low("p")
    d = c.dim()
    core = "param %s: %s = %s;\nconst node %s: %s = @%s;" % (
        p, d, c.lit(d), c.low("bad"), d, p,
    )
    return core, "const node references a runtime param"


@template
def t_assert_not_bool(c: Ctx) -> Case:
    d = c.dim()
    core = "assert %s = %s;" % (c.low("as"), c.lit(d))
    return core, "assert body is not Bool"


@template
def t_tolerance_dim_mismatch(c: Ctx) -> Case:
    d1, d2 = c.dim2()
    a, b = c.low("a"), c.low("b")
    core = "param %s: %s = %s;\nparam %s: %s = %s;\nassert %s = @%s ~= @%s +/- %s;" % (
        a, d1, c.lit(d1), b, d1, c.lit(d1), c.low("as"), a, b, c.lit(d2),
    )
    return core, "tolerance of dimension %s for %s values" % (d2, d1)


@template
def t_negative_tolerance(c: Ctx) -> Case:
    d = c.dim()
    a, b = c.low("a"), c.low("b")
    tol = c.lit(d)
    core = "param %s: %s = %s;\nparam %s: %s = %s;\nassert %s = @%s ~= @%s +/- -%s;" % (
        a, d, c.lit(d), b, d, c.lit(d), c.low("as"), a, b, tol,
    )
    return core, "negative tolerance"


@template
def t_pub_param(c: Ctx) -> Case:
    d = c.dim()
    core = "%s param %s: %s = %s;" % (
        c.r.choice(["pub", "pub(bind)"]), c.low("p"), d, c.lit(d),
    )
    return core, "visibility marker on a param"


@template
def t_unknown_attribute(c: Ctx) -> Case:
    d = c.dim()
    core = "#[%s]\nnode %s: %s = %s;" % (c.low("attr").replace("_", ""), c.low("bad"), d, c.lit(d))
    return core, "unrecognized attribute"


@template
def t_assumes_unknown(c: Ctx) -> Case:
    d = c.dim()
    core = "#[assumes(%s)]\nnode %s: %s = %s;" % (c.low("ghost"), c.low("bad"), d, c.lit(d))
    return core, "#[assumes] naming an undeclared assert"


@template
def t_expected_fail_on_node(c: Ctx) -> Case:
    d = c.dim()
    core = "#[expected_fail]\nnode %s: %s = %s;" % (c.low("bad"), d, c.lit(d))
    return core, "#[expected_fail] on a node"


@template
def t_dag_scope_leak(c: Ctx) -> Case:
    k = c.low("k")
    g = c.low("dg")
    d = c.dim()
    inner = c.low("res")
    core = (
        "const node %s: %s = %s;\ndag %s {\n    pub node %s: %s = @%s;\n}\n"
        "include %s().{ %s };" % (k, d, c.lit(d), g, inner, d, k, g, inner)
    )
    return core, "dag body references a top-level declaration without importing it"


@template
def t_include_unknown_binding(c: Ctx) -> Case:
    g = c.low("dg")
    d = c.dim()
    p, res = c.low("p"), c.low("res")
    core = (
        "dag %s {\n    param %s: %s;\n    pub node %s: %s = @%s;\n}\n"
        "include %s(%s: %s, %s: %s).{ %s };"
        % (g, p, d, res, d, p, g, p, c.lit(d), c.low("ghost"), c.lit(d), res)
    )
    return core, "include binds a param the dag does not declare"


@template
def t_include_missing_param(c: Ctx) -> Case:
    g = c.low("dg")
    d = c.dim()
    p, q, res = c.low("p"), c.low("q"), c.low("res")
    core = (
        "dag %s {\n    param %s: %s;\n    param %s: %s;\n    pub node %s: %s = @%s + @%s;\n}\n"
        "include %s(%s: %s).{ %s };"
        % (g, p, d, q, d, res, d, p, q, g, p, c.lit(d), res)
    )
    return core, "include omits the required param %s" % q


@template
def t_include_unknown_output(c: Ctx) -> Case:
    g = c.low("dg")
    d = c.dim()
    p, res = c.low("p"), c.low("res")
    core = (
        "dag %s {\n    param %s: %s;\n    pub node %s: %s = @%s;\n}\n"
        "include %s(%s: %s).{ %s };" % (g, p, d, res, d, p, g, p, c.lit(d), c.low("ghost"))
    )
    return core, "include selects an output the dag does not export"


@template
def t_include_private_output(c: Ctx) -> Case:
    g = c.low("dg")
    d = c.dim()
    p, res = c.low("p"), c.low("res")
    core = (
        "dag %s {\n    param %s: %s;\n    node %s: %s = @%s;\n}\n"
        "include %s(%s: %s).{ %s };" % (g, p, d, res, d, p, g, p, c.lit(d), res)
    )
    return core, "include selects a private node"


@template
def t_include_binding_dim_mismatch(c: Ctx) -> Case:
    g = c.low("dg")
    d1, d2 = c.dim2()
    p, res = c.low("p"), c.low("res")
    core = (
        "dag %s {\n    param %s: %s;\n    pub node %s: %s = @%s;\n}\n"
        "include %s(%s: %s).{ %s };" % (g, p, d1, res, d1, p, g, p, c.lit(d2), res)
    )
    return core, "include binds %s to a %s param" % (d2, d1)


@template
def t_dag_call_in_const(c: Ctx) -> Case:
    g = c.low("dg")
    d = c.dim()
    p, res = c.low("p"), c.low("res")
    core = (
        "dag %s {\n    param %s: %s;\n    pub node %s: %s = @%s;\n}\n"
        "const node %s: %s = @%s(%s: %s).%s;"
        % (g, p, d, res, d, p, c.low("bad"), d, g, p, c.lit(d), res)
    )
    return core, "dag call inside a const node"


@template
def t_domain_violation(c: Ctx) -> Case:
    d = c.dim()
    u = c.unit(d)
    lo, hi = 100.0, 200.0
    if u is None:
        lo_s, hi_s, val_s = "100.0", "200.0", "%s" % c.num(300, 400)
    else:
        lo_s = "%.1f %s" % (lo, u)
        hi_s = "%.1f %s" % (hi, u)
        val_s = "%.1f %s" % (c.r.uniform(300, 400), u)
    core = "param %s: %s(min: %s, max: %s) = %s;" % (c.low("p"), d, lo_s, hi_s, val_s)
    return core, "default value outside the declared domain"


@template
def t_domain_min_gt_max(c: Ctx) -> Case:
    d = c.dim()
    u = c.unit(d)
    if u is None:
        lo_s, hi_s, val_s = "500.0", "100.0", "200.0"
    else:
        lo_s, hi_s, val_s = "500.0 %s" % u, "100.0 %s" % u, "200.0 %s" % u
    core = "param %s: %s(min: %s, max: %s) = %s;" % (c.low("p"), d, lo_s, hi_s, val_s)
    return core, "domain min exceeds max"


@template
def t_domain_on_bool(c: Ctx) -> Case:
    kind = c.r.choice(["Bool", "bool_max"])
    if kind == "Bool":
        core = "param %s: Bool(min: false) = true;" % c.low("p")
    else:
        core = "param %s: Bool(max: true) = false;" % c.low("p")
    return core, "domain constraint on Bool"


@template
def t_domain_bound_dim_mismatch(c: Ctx) -> Case:
    d1, d2 = c.dim2()
    core = "param %s: %s(min: %s) = %s;" % (c.low("p"), d1, c.lit(d2), c.lit(d1))
    return core, "domain bound of dimension %s on a %s declaration" % (d2, d1)


@template
def t_domain_bound_runtime(c: Ctx) -> Case:
    d = c.dim()
    p = c.low("p")
    core = "param %s: %s = %s;\nparam %s: %s(min: @%s) = %s;" % (
        p, d, c.lit(d), c.low("bad"), d, p, c.lit(d),
    )
    return core, "domain bound referencing a runtime param"


@template
def t_unknown_name(c: Ctx) -> Case:
    kind = c.r.choice(["dim", "unit", "index", "type"])
    if kind == "dim":
        core = "node %s: %s = %s;" % (c.low("bad"), c.up("Ghost"), c.num())
        return core, "unknown dimension in an annotation"
    if kind == "unit":
        core = "node %s: Length = %s %s;" % (c.low("bad"), c.num(), c.low("gh"))
        return core, "unknown unit in a quantity literal"
    if kind == "index":
        d = c.dim()
        core = "node %s: %s[%s] = %s;" % (c.low("bad"), d, c.up("Ghost"), c.lit(d))
        return core, "unknown index in an annotation"
    core = "node %s: %s = %s(%s: %s);" % (
        c.low("bad"), c.up("Ghost"), c.up("Ghost"), c.low("f"), c.num(),
    )
    return core, "unknown type / constructor"


@template
def t_unknown_import(c: Ctx) -> Case:
    core = "import %s.%s;\nnode %s: Dimensionless = %s;" % (
        c.low("pkg"), c.low("mod"), c.low("bad"), c.num(),
    )
    return core, "import of a module that does not exist"


@template
def t_type_empty_body(c: Ctx) -> Case:
    core = "type %s {}\nnode %s: Dimensionless = %s;" % (c.up("Em"), c.low("bad"), c.num())
    return core, "empty type body"


@template
def t_unit_no_body_without_base(c: Ctx) -> Case:
    core = "unit %s: Length;" % c.low("u")
    return core, "non-base unit declared without a body"


@template
def t_field_only_type_body(c: Ctx) -> Case:
    core = "type %s {\n    %s: Length,\n}" % (c.up("Rc"), c.low("f"))
    return core, "field-only type body (constructor name required)"


# --- datetime --------------------------------------------------------------

SCALES = ["UTC", "TAI", "TT", "TDB", "ET", "GPST", "GST", "BDT", "QZSST"]


@template
def t_datetime_cross_scale(c: Ctx) -> Case:
    s1, s2 = c.r.sample(SCALES[1:], 2)
    a, b = c.low("t"), c.low("t")
    core = (
        'param %s: Datetime<%s> = epoch<%s>("2025-0%d-1%dT0%d:00:00");\n'
        'param %s: Datetime<%s> = epoch<%s>("2025-0%d-1%dT0%d:00:00");\n'
        "node %s: Time = @%s - @%s;"
        % (a, s1, s1, c.r.randint(1, 9), c.r.randint(0, 9), c.r.randint(0, 9),
           b, s2, s2, c.r.randint(1, 9), c.r.randint(0, 9), c.r.randint(0, 9),
           c.low("bad"), a, b)
    )
    return core, "subtraction across time scales %s and %s" % (s1, s2)


@template
def t_datetime_addition(c: Ctx) -> Case:
    a = c.low("t")
    core = (
        'param %s: Datetime = datetime("2025-03-1%dT0%d:00:00Z");\n'
        "node %s: Datetime = @%s + @%s;"
        % (a, c.r.randint(0, 9), c.r.randint(0, 9), c.low("bad"), a, a)
    )
    return core, "Datetime + Datetime"


@template
def t_time_minus_datetime(c: Ctx) -> Case:
    a, b = c.low("t"), c.low("d")
    core = (
        'param %s: Datetime = datetime("2025-04-1%dT1%d:00:00Z");\n'
        "param %s: Time = %s;\nnode %s: Datetime = @%s - @%s;"
        % (a, c.r.randint(0, 9), c.r.randint(0, 9), b, c.lit("Time"), c.low("bad"), b, a)
    )
    return core, "Time - Datetime"


@template
def t_datetime_invalid_literal(c: Ctx) -> Case:
    form = c.r.choice(["date", "dateonly", "nooffset", "positional", "month"])
    if form == "date":
        lit = 'datetime("2025-02-30T00:00:00Z")'
        why = "invalid calendar date"
    elif form == "dateonly":
        lit = 'datetime("2025-05-1%d")' % c.r.randint(0, 9)
        why = "date-only literal"
    elif form == "nooffset":
        lit = 'datetime("2025-06-1%dT12:00:00")' % c.r.randint(0, 9)
        why = "civil literal without an offset"
    elif form == "positional":
        lit = 'epoch("2025-07-1%dT00:00:00", TT)' % c.r.randint(0, 9)
        why = "obsolete positional epoch scale"
    else:
        lit = 'datetime("2025-1%d-05T00:00:00Z")' % c.r.randint(3, 9)
        why = "month out of range"
    core = "param %s: Datetime = %s;" % (c.low("t"), lit)
    return core, why


@template
def t_datetime_extract_non_datetime(c: Ctx) -> Case:
    fn = c.r.choice(["year", "month", "day", "hour", "minute", "second", "weekday"])
    core = "node %s: Int = %s(%s);" % (c.low("bad"), fn, c.lit(c.dim()))
    return core, "%s applied to a quantity" % fn


@template
def t_datetime_scale_annotation(c: Ctx) -> Case:
    s1, s2 = c.r.sample(SCALES[1:], 2)
    core = 'param %s: Datetime<%s> = epoch<%s>("2025-08-0%dT06:00:00");' % (
        c.low("t"), s1, s2, c.r.randint(1, 9),
    )
    return core, "Datetime<%s> annotation with an epoch<%s> value" % (s1, s2)


# --- complex ---------------------------------------------------------------


@template
def t_complex_ordering(c: Ctx) -> Case:
    d = c.dim()
    a = c.low("z")
    core = "node %s: Complex<%s> = complex(%s, %s);\nassert %s = @%s %s @%s;" % (
        a, d, c.lit(d), c.lit(d), c.low("as"), a, c.r.choice(["<", ">", "<=", ">="]), a,
    )
    return core, "ordering comparison on complex values"


@template
def t_complex_mixed_add(c: Ctx) -> Case:
    d = c.dim()
    z = c.low("z")
    core = "node %s: Complex<%s> = complex(%s, %s);\nnode %s: Complex<%s> = @%s + %s;" % (
        z, d, c.lit(d), c.lit(d), c.low("bad"), d, z, c.lit(d),
    )
    return core, "complex plus real without to_complex"


@template
def t_complex_bad_generic(c: Ctx) -> Case:
    form = c.r.choice(["unit", "bare", "int"])
    if form == "unit":
        ann = "Complex<%s>" % c.r.choice(["m", "s", "kg"])
        why = "unit argument to Complex"
    elif form == "bare":
        ann = "Complex"
        why = "bare Complex without a dimension argument"
    else:
        ann = "Complex<%d>" % c.r.randint(2, 5)
        why = "Nat argument to Complex"
    core = "node %s: %s = complex(%s, %s);" % (c.low("bad"), ann, c.num(), c.num())
    return core, why


@template
def t_complex_exp_dimensioned(c: Ctx) -> Case:
    d = c.dim()
    z = c.low("z")
    core = "node %s: Complex<%s> = complex(%s, %s);\nnode %s: Complex<%s> = exp(@%s);" % (
        z, d, c.lit(d), c.lit(d), c.low("bad"), d, z,
    )
    return core, "exp of a dimensioned complex value"


@template
def t_complex_domain(c: Ctx) -> Case:
    d = c.dim()
    core = "param %s: Complex<%s>(min: %s) = complex(%s, %s);" % (
        c.low("p"), d, c.lit(d), c.lit(d), c.lit(d),
    )
    return core, "domain constraint on a complex type"


# --- plots / figures / layers ---------------------------------------------

MARKS = ["point", "line", "bar", "area", "rect", "tick"]


@template
def t_plot_missing_field(c: Ctx) -> Case:
    ix, labels, decl = _index_decl(c, 2)
    d = c.dim()
    val = c.low("val")
    entries = ", ".join("%s.%s: %s" % (ix, l, c.lit(d)) for l in labels)
    body = "encode: { x: @%s }" % val if c.r.random() < 0.5 else "mark: %s" % c.r.choice(MARKS)
    core = "%s\nnode %s: %s[%s] = { %s };\nplot %s = {\n    %s,\n};" % (
        decl, val, d, ix, entries, c.low("pl"), body,
    )
    return core, "plot without both a mark and a non-empty encode block"


@template
def t_plot_duplicate_field(c: Ctx) -> Case:
    ix, labels, decl = _index_decl(c, 2)
    d = c.dim()
    val = c.low("val")
    entries = ", ".join("%s.%s: %s" % (ix, l, c.lit(d)) for l in labels)
    m1, m2 = c.r.sample(MARKS, 2)
    core = (
        "%s\nnode %s: %s[%s] = { %s };\nplot %s = {\n    mark: %s,\n    mark: %s,\n"
        "    encode: { y: @%s },\n};" % (decl, val, d, ix, entries, c.low("pl"), m1, m2, val)
    )
    return core, "duplicate `mark` field in a plot"


@template
def t_plot_unknown_mark(c: Ctx) -> Case:
    ix, labels, decl = _index_decl(c, 2)
    d = c.dim()
    val = c.low("val")
    entries = ", ".join("%s.%s: %s" % (ix, l, c.lit(d)) for l in labels)
    core = "%s\nnode %s: %s[%s] = { %s };\nplot %s = {\n    mark: %s,\n    encode: { y: @%s },\n};" % (
        decl, val, d, ix, entries, c.low("pl"), c.low("scatter").replace("_", ""), val,
    )
    return core, "unknown mark type"


@template
def t_plot_axis_mismatch(c: Ctx) -> Case:
    ix, ilabels, idecl = _index_decl(c, 2)
    jx, jlabels, jdecl = _index_decl(c, 3)
    d1, d2 = c.dim2()
    a, b = c.low("xs"), c.low("ys")
    core = (
        "%s\n%s\nnode %s: %s[%s] = for %s: %s { %s };\nnode %s: %s[%s] = for %s: %s { %s };\n"
        "plot %s = {\n    mark: %s,\n    encode: { x: @%s, y: @%s },\n};"
        % (idecl, jdecl, a, d1, ix, "u1", ix, c.lit(d1), b, d2, jx, "u2", jx, c.lit(d2),
           c.low("pl"), c.r.choice(MARKS), a, b)
    )
    return core, "plot channels indexed by different axes"


@template
def t_figure_unknown_plot(c: Ctx) -> Case:
    kind = c.r.choice(["figure", "layer"])
    core = "%s %s = {\n    plots: [%s],\n};" % (kind, c.low("fg"), c.low("ghost"))
    return core, "%s references an undeclared plot" % kind


# --- imports of self-package ----------------------------------------------


@template
def t_import_runtime_item(c: Ctx) -> Case:
    d = c.dim()
    p = c.low("p")
    core = "param %s: %s = %s;\nimport %s.{ %s };" % (p, d, c.lit(d), c.stem, p)
    return core, "import of a runtime param"


@template
def t_import_unknown_item(c: Ctx) -> Case:
    core = "import %s.{ %s };\nnode %s: Dimensionless = %s;" % (
        c.stem, c.low("ghost"), c.low("bad"), c.num(),
    )
    return core, "import of a name the module does not declare"


# --- keys, recurrences ----------------------------------------------------


@template
def t_label_as_scalar(c: Ctx) -> Case:
    ix, labels, decl = _index_decl(c, 2)
    core = "%s\nnode %s: %s = %s.%s;" % (
        decl, c.low("bad"), c.r.choice(["Dimensionless", "Int"]), ix, labels[0],
    )
    return core, "index label bound to a scalar type"


@template
def t_key_match_fin(c: Ctx) -> Case:
    n = c.r.randint(2, 4)
    core = "node %s: Dimensionless = match key(Fin(%d), 0) {\n%s\n};" % (
        c.low("bad"), n, "\n".join("    #%d => %s," % (i, c.num()) for i in range(n)),
    )
    return core, "match over a Fin-axis key"


@template
def t_unfold_first_arg(c: Ctx) -> Case:
    d = "Dimensionless"
    a = c.low("a")
    core = (
        "param %s: %s = %s;\nnode %s: %s = unfold(@%s, %s, |prev, pi, i| prev + %s);"
        % (a, d, c.num(), c.low("bad"), d, a, c.num(), c.num())
    )
    return core, "unfold over a value instead of a coordinate index"


@template
def t_scan_closure_arity(c: Ctx) -> Case:
    ix, labels, decl = _index_decl(c, 3)
    d = "Dimensionless"
    src = c.low("src")
    entries = ", ".join("%s.%s: %s" % (ix, l, c.num()) for l in labels)
    core = "%s\nnode %s: %s[%s] = { %s };\nnode %s: %s[%s] = scan(@%s, %s, |acc| acc);" % (
        decl, src, d, ix, entries, c.low("bad"), d, ix, src, c.num(),
    )
    return core, "scan closure with one binder"


@template
def t_linalg_square_required(c: Ctx) -> Case:
    fn = c.r.choice(["det", "inverse", "solve"])
    n = c.r.randint(2, 3)
    m = n + 1
    a, b = c.low("a"), c.low("b")
    mat = (
        "param %s: Dimensionless[Fin(%d), Fin(%d)] = for i: Fin(%d), j: Fin(%d) { %s };"
        % (a, n, m, n, m, c.num(1, 5))
    )
    if fn == "det":
        core = "%s\nnode %s: Dimensionless = det(@%s);" % (mat, c.low("bad"), a)
    elif fn == "inverse":
        core = "%s\nnode %s: Dimensionless[Fin(%d), Fin(%d)] = inverse(@%s);" % (
            mat, c.low("bad"), n, m, a,
        )
    else:
        core = (
            "%s\nparam %s: Dimensionless[Fin(%d)] = for i: Fin(%d) { %s };\n"
            "node %s: Dimensionless[Fin(%d)] = solve(@%s, @%s);"
            % (mat, b, n, n, c.num(1, 5), c.low("bad"), n, a, b)
        )
    return core, "%s requires a square typed-axis matrix" % fn


# --- domains, attributes, generics ----------------------------------------


@template
def t_int_domain_bound_float(c: Ctx) -> Case:
    core = "param %s: Int(%s: %s) = %d;" % (
        c.low("p"), c.r.choice(["min", "max"]), c.num(1, 9), c.r.randint(1, 5),
    )
    return core, "non-integer bound on an Int domain"


@template
def t_domain_unknown_key(c: Ctx) -> Case:
    d = c.dim()
    core = "param %s: %s(%s: %s) = %s;" % (
        c.low("p"), d, c.r.choice(["step", "scale", "mid"]), c.lit(d), c.lit(d),
    )
    return core, "unknown domain constraint key"


@template
def t_domain_field_min_gt_max(c: Ctx) -> Case:
    t = c.up("St")
    f = c.low("f")
    d = c.dim()
    u = c.unit(d)
    lo, hi = ("500.0", "100.0") if u is None else ("500.0 %s" % u, "100.0 %s" % u)
    val = "200.0" if u is None else "200.0 %s" % u
    core = "type %s {\n    %s(%s: %s(min: %s, max: %s)),\n}\nnode %s: %s = %s(%s: %s);" % (
        t, t, f, d, lo, hi, c.low("bad"), t, t, f, val,
    )
    return core, "constructor field domain with min > max"


@template
def t_datetime_domain_scale_mismatch(c: Ctx) -> Case:
    s = c.r.choice(["TT", "TAI", "TDB", "GPST"])
    core = (
        'param %s: Datetime<%s>(min: datetime("2025-01-0%dT00:00:00Z")) = '
        'epoch<%s>("2025-06-0%dT12:00:00");'
        % (c.low("t"), s, c.r.randint(1, 9), s, c.r.randint(1, 9))
    )
    return core, "UTC bound on a Datetime<%s> domain" % s


@template
def t_expected_fail_misuse(c: Ctx) -> Case:
    ix, labels, decl = _index_decl(c, 2)
    form = c.r.choice(["variant_on_scalar", "position_on_named", "position_oob"])
    if form == "variant_on_scalar":
        core = "%s\n#[expected_fail(%s.%s)]\nassert %s = %s < %s;" % (
            decl, ix, labels[0], c.low("as"), c.num(1, 5), c.num(0.1, 0.9),
        )
        return core, "per-variant #[expected_fail] on a scalar assert"
    d = c.dim()
    val = c.low("val")
    entries = ", ".join("%s.%s: %s" % (ix, l, c.lit(d)) for l in labels)
    if form == "position_on_named":
        core = "%s\nnode %s: %s[%s] = { %s };\n#[expected_fail(#0)]\nassert %s = @%s == %s;" % (
            decl, val, d, ix, entries, c.low("as"), val, c.lit(d),
        )
        return core, "positional #[expected_fail] key on a named axis"
    n = c.r.randint(2, 4)
    core = (
        "node %s: %s[Fin(%d)] = for %s: Fin(%d) { %s };\n#[expected_fail(#%d)]\n"
        "assert %s = @%s == %s;"
        % (val, d, n, "hh", n, c.lit(d), n + c.r.randint(0, 3), c.low("as"), val, c.lit(d))
    )
    return core, "positional #[expected_fail] key out of bounds"


@template
def t_assumes_on_const(c: Ctx) -> Case:
    d = c.dim()
    a = c.low("as")
    core = "assert %s = %s < %s;\n#[assumes(%s)]\nconst node %s: %s = %s;" % (
        a, c.num(1, 3), c.num(10, 20), a, c.low("bad"), d, c.lit(d),
    )
    return core, "#[assumes] on a const node"


@template
def t_generic_default_not_trailing(c: Ctx) -> Case:
    t = c.up("Gd")
    core = "type %s<A: Dim = Length, B: Dim> {\n    %s(x: A, y: B),\n}" % (t, t)
    return core, "defaulted generic parameter before a non-defaulted one"


@template
def t_generic_default_forward_ref(c: Ctx) -> Case:
    t = c.up("Gd")
    core = "type %s<A: Dim = B, B: Dim = Time> {\n    %s(x: A, y: B),\n}" % (t, t)
    return core, "generic default referring to a later parameter"


@template
def t_nat_subtraction(c: Ctx) -> Case:
    d = c.dim()
    core = "node %s: %s[Fin(%d - 1)] = for %s: Fin(%d) { %s };" % (
        c.low("bad"), d, c.r.randint(3, 6), "kk", c.r.randint(2, 5), c.lit(d),
    )
    return core, "Nat subtraction in a Fin extent"


# --- conditions, patterns, misc -------------------------------------------


@template
def t_if_condition_non_bool(c: Ctx) -> Case:
    d = c.dim()
    a = c.low("a")
    core = "param %s: %s = %s;\nnode %s: %s = if @%s { %s } else { %s };" % (
        a, d, c.lit(d), c.low("bad"), d, a, c.lit(d), c.lit(d),
    )
    return core, "non-Bool condition in if/else"


@template
def t_bool_arithmetic(c: Ctx) -> Case:
    f = c.low("flag")
    core = "param %s: Bool = true;\nnode %s: Dimensionless = @%s %s %s;" % (
        f, c.low("bad"), f, c.r.choice(["+", "*", "-"]), c.num(),
    )
    return core, "arithmetic on a Bool operand"


@template
def t_bool_ordering(c: Ctx) -> Case:
    f, g = c.low("flag"), c.low("flag")
    core = "param %s: Bool = true;\nparam %s: Bool = false;\nassert %s = @%s %s @%s;" % (
        f, g, c.low("as"), f, c.r.choice(["<", ">", "<=", ">="]), g,
    )
    return core, "ordering comparison on Bool operands"


@template
def t_match_pattern_field_mismatch(c: Ctx) -> Case:
    t = c.up("Un")
    v = c.up("Vv")
    f1, f2 = c.low("f"), c.low("f")
    d1, d2 = c.dim2()
    val = c.low("val")
    form = c.r.choice(["missing", "extra", "unknown"])
    if form == "missing":
        pattern = "%s(%s: got)" % (v, f1)
        why = "match pattern omits field %s" % f2
    elif form == "extra":
        pattern = "%s(%s: got, %s: _, %s: _)" % (v, f1, f2, c.low("zz"))
        why = "match pattern binds a field the constructor lacks"
    else:
        pattern = "%s(%s: got, %s: _)" % (v, c.low("zz"), f2)
        why = "match pattern names an unknown field"
    core = (
        "type %s {\n    %s(%s: %s, %s: %s),\n}\nnode %s: %s = %s(%s: %s, %s: %s);\n"
        "node %s: %s = match @%s {\n    %s => got,\n};"
        % (t, v, f1, d1, f2, d2, val, t, v, f1, c.lit(d1), f2, c.lit(d2), c.low("bad"),
           d1, val, pattern)
    )
    return core, why


@template
def t_timezone_on_non_datetime(c: Ctx) -> Case:
    d = c.dim()
    a = c.low("a")
    core = 'param %s: %s = %s;\nnode %s: %s = @%s -> "America/New_York";' % (
        a, d, c.lit(d), c.low("bad"), d, a,
    )
    return core, "timezone display target on a quantity"


@template
def t_invalid_timezone(c: Ctx) -> Case:
    t = c.low("t")
    core = (
        'param %s: Datetime = datetime("2025-09-0%dT08:00:00Z");\n'
        'node %s: Datetime = @%s -> "%s/%s";'
        % (t, c.r.randint(1, 9), c.low("bad"), t, c.up("Nowhere"), c.up("Nocity"))
    )
    return core, "unknown IANA timezone"


@template
def t_complex_aggregation(c: Ctx) -> Case:
    ix, labels, decl = _index_decl(c, 2)
    d = c.dim()
    val = c.low("val")
    entries = ", ".join(
        "%s.%s: complex(%s, %s)" % (ix, l, c.lit(d), c.lit(d)) for l in labels
    )
    fn = c.r.choice(["sum", "maximum", "mean", "argmax"])
    core = "%s\nnode %s: Complex<%s>[%s] = { %s };\nnode %s: Complex<%s> = %s(@%s);" % (
        decl, val, d, ix, entries, c.low("bad"), d, fn, val,
    )
    return core, "%s over complex values" % fn


@template
def t_count_multi_axis(c: Ctx) -> Case:
    ix, ilabels, idecl = _index_decl(c, 2)
    jx, jlabels, jdecl = _index_decl(c, 2)
    d = c.dim()
    val = c.low("val")
    core = "%s\n%s\nnode %s: %s[%s, %s] = for %s: %s, %s: %s { %s };\nnode %s: Int = count(@%s);" % (
        idecl, jdecl, val, d, ix, jx, "m1", ix, "m2", jx, c.lit(d), c.low("bad"), val,
    )
    return core, "count over a two-axis value"


@template
def t_nested_indexed_map(c: Ctx) -> Case:
    ix, ilabels, idecl = _index_decl(c, 2)
    jx, jlabels, jdecl = _index_decl(c, 2)
    d = c.dim()
    inner = "{ %s }" % ", ".join("%s.%s: %s" % (jx, l, c.lit(d)) for l in jlabels)
    entries = ",\n".join("    %s.%s: %s" % (ix, l, inner) for l in ilabels)
    core = "%s\n%s\nnode %s: %s[%s] = {\n%s,\n};" % (
        idecl, jdecl, c.low("bad"), d, ix, entries,
    )
    return core, "map literal whose entries are themselves indexed"


@template
def t_dag_call_in_domain_bound(c: Ctx) -> Case:
    g = c.low("dg")
    d = c.dim()
    p, res = c.low("p"), c.low("res")
    core = (
        "dag %s {\n    param %s: %s;\n    pub node %s: %s = @%s;\n}\n"
        "param %s: %s(min: @%s(%s: %s).%s) = %s;"
        % (g, p, d, res, d, p, c.low("bad"), d, g, p, c.lit(d), res, c.lit(d))
    )
    return core, "dag call inside a domain bound"


@template
def t_table_fin_axis_labels(c: Ctx) -> Case:
    n = c.r.randint(2, 3)
    d = c.dim()
    rows = "\n".join("    %s: %s;" % (c.up("Lb"), c.lit(d)) for _ in range(n))
    core = "param %s: %s[Fin(%d)] = table[Fin(%d)] {\n%s\n};" % (
        c.low("bad"), d, n, n, rows,
    )
    return core, "row labels on a Fin table axis"


@template
def t_table_named_axis_no_labels(c: Ctx) -> Case:
    ix, labels, decl = _index_decl(c, 2)
    d = c.dim()
    rows = "\n".join("    %s;" % c.lit(d) for _ in labels)
    core = "%s\nparam %s: %s[%s] = table[%s] {\n%s\n};" % (
        decl, c.low("bad"), d, ix, ix, rows,
    )
    return core, "named table axis without row labels"


# ---------------------------------------------------------------------------


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("-n", "--count", type=int, default=600)
    ap.add_argument("-o", "--out", default="cases")
    ap.add_argument("-s", "--seed", type=int, default=20260817)
    args = ap.parse_args()

    if os.path.isdir(args.out):
        shutil.rmtree(args.out)
    os.makedirs(args.out)

    master = random.Random(args.seed)
    manifest = []
    per = {}
    for i in range(args.count):
        tpl = TEMPLATES[i % len(TEMPLATES)] if i < len(TEMPLATES) else master.choice(TEMPLATES)
        seed = master.randrange(1 << 40)
        rng = random.Random(seed)
        name = tpl.__name__[2:]
        per[name] = per.get(name, 0) + 1
        stem = "%s_%03d" % (name, per[name])
        c = Ctx(rng, stem)
        core, reason = tpl(c)
        src = assemble(c, core)
        header = "// %s\n// expected: rejected -- %s\n" % (stem, reason)
        with open(os.path.join(args.out, stem + ".gcl"), "w") as fh:
            fh.write(header + src)
        manifest.append((stem, name, reason))

    with open(os.path.join(args.out, "MANIFEST.tsv"), "w") as fh:
        for stem, name, reason in manifest:
            fh.write("%s\t%s\t%s\n" % (stem, name, reason))
    print("templates: %d, files: %d -> %s" % (len(TEMPLATES), len(manifest), args.out))


if __name__ == "__main__":
    main()
