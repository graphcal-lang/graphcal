#!/usr/bin/env python3
"""High-entropy generator of Graphcal files that SHOULD fail `graphcal check`.

Design:
  * ~110 probe families. Each instantiation produces an *invalid* probe file
    embedding exactly ONE intended compile-time error, plus (where meaningful)
    a minimally-different *control* file that must PASS check. Controls verify
    that the surrounding scaffold/idiom is valid, so a passing probe indicates
    the specific validation is missing (a compiler bug), not a broken scaffold.
  * Random filler declarations (valid, namespaced) + declaration-order
    shuffling + randomized identifiers/literals give high entropy.
  * A manifest.jsonl records file, family, expected outcome, reason, confidence.

Confidence:
  * high  -> the docs explicitly say this must be a compile-time error.
  * med   -> strongly implied by docs/design principles, not spelled out.
"""

import json
import math
import os
import random
import shutil
import sys

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "corpus")
SEED = int(os.environ.get("GEN_SEED", "20260817"))

# ----------------------------------------------------------------- utilities

SYLL = ["ax", "bor", "cel", "dov", "eph", "fyn", "gor", "hux", "ith", "jaz",
        "kel", "lom", "myr", "nix", "oth", "pyx", "quo", "rel", "syn", "tor",
        "urv", "vex", "wyn", "xen", "yol", "zeb", "fla", "gru", "ple", "sni"]

_counter = [0]


def fresh(rng, kind="value"):
    """Unique random identifier. kind: value|Type"""
    _counter[0] += 1
    core = "".join(rng.choice(SYLL) for _ in range(rng.randint(1, 3)))
    tag = f"{_counter[0]:x}"
    if kind == "Type":
        return core.capitalize() + rng.choice(["", "X", "Q", "Vr", "Kt"]) + tag.upper()
    sep = rng.choice(["_", "_", ""])
    return core + sep + tag


def flit(rng, lo=0.1, hi=999.0):
    """Random float literal text (always a valid graphcal float literal)."""
    v = rng.uniform(lo, hi)
    style = rng.random()
    if style < 0.25:
        return f"{v:.1f}"
    if style < 0.5:
        return f"{v:.4f}"
    if style < 0.7:
        return f"{v:.6g}" if ("e" in f"{v:.6g}" or "." in f"{v:.6g}") else f"{v:.1f}"
    if style < 0.85:
        return f"{v * rng.choice([1e-3, 1e3, 1e6]):.3e}"
    return f"{v:.2f}"


def ilit(rng, lo=1, hi=5000):
    v = rng.randint(lo, hi)
    if rng.random() < 0.15 and v > 1000:
        s = str(v)
        return s[:-3] + "_" + s[-3:]
    return str(v)


# dimension name -> (canonical signature, [unit spellings])
DIMS = {
    "Length": ("L", ["m", "km", "cm", "mm"]),
    "Time": ("T", ["s", "min", "h"]),
    "Mass": ("M", ["kg", "g"]),
    "Temperature": ("Th", ["K"]),
    "ElectricCurrent": ("I", ["A"]),
    "Amount": ("N", ["mol"]),
    "LuminousIntensity": ("J", ["cd"]),
    "Angle": ("A", ["rad", "deg"]),
    "Force": ("MLT-2", ["N", "kN"]),
    "Energy": ("ML2T-2", ["J", "kJ"]),
    "Power": ("ML2T-3", ["W", "kW"]),
    "Pressure": ("ML-1T-2", ["Pa", "kPa", "MPa"]),
    "Frequency": ("T-1", ["Hz"]),
    "Velocity": ("LT-1", ["m/s", "km/s", "km/h"]),
    "Acceleration": ("LT-2", ["m/s^2"]),
    "Area": ("L2", ["m^2"]),
    "Volume": ("L3", ["m^3"]),
}

BASE_DIMS = ["Length", "Time", "Mass", "Temperature", "ElectricCurrent",
             "Amount", "LuminousIntensity", "Angle"]


def pick_dim(rng, exclude=()):
    ex_sigs = {DIMS[d][0] for d in exclude if d in DIMS}
    while True:
        d = rng.choice(list(DIMS))
        if d not in exclude and DIMS[d][0] not in ex_sigs:
            return d


def unit_of(rng, dim):
    return rng.choice(DIMS[dim][1])


def qlit(rng, dim, lo=0.1, hi=999.0):
    """Quantity literal of the given dimension."""
    return f"{flit(rng, lo, hi)} {unit_of(rng, dim)}"


class Ctx:
    """Per-file random context with helper shortcuts."""

    def __init__(self, rng):
        self.rng = rng

    def v(self):
        return fresh(self.rng, "value")

    def T(self):
        return fresh(self.rng, "Type")

    def dim2(self):
        d1 = pick_dim(self.rng)
        d2 = pick_dim(self.rng, exclude=(d1,))
        return d1, d2

    def q(self, dim, lo=0.1, hi=999.0):
        return qlit(self.rng, dim, lo, hi)

    def f(self, lo=0.1, hi=999.0):
        return flit(self.rng, lo, hi)

    def u(self, dim):
        return unit_of(self.rng, dim)


# ------------------------------------------------------------ valid fillers

def filler_decls(rng, n):
    """n independent, definitely-valid declaration groups."""
    out = []
    for _ in range(n):
        kind = rng.randrange(10)
        p = fresh(rng)
        if kind == 0:
            d = pick_dim(rng)
            out.append(f"param {p}: {d} = {qlit(rng, d)};")
        elif kind == 1:
            out.append(f"const node {p}: Dimensionless = {flit(rng)};")
        elif kind == 2:
            d = pick_dim(rng)
            q = fresh(rng)
            out.append(f"param {p}: {d} = {qlit(rng, d)};\n"
                       f"node {q}: {d} = @{p} * {flit(rng, 0.2, 4.0)};")
        elif kind == 3:
            x = fresh(rng, "Type")
            a, b, c = fresh(rng, "Type"), fresh(rng, "Type"), fresh(rng, "Type")
            m = fresh(rng)
            s = fresh(rng)
            d = pick_dim(rng)
            out.append(
                f"index {x} = {{ {a}, {b}, {c} }};\n"
                f"node {m}: {d}[{x}] = {{ {x}.{a}: {qlit(rng, d)}, "
                f"{x}.{b}: {qlit(rng, d)}, {x}.{c}: {qlit(rng, d)} }};\n"
                f"node {s}: {d} = sum(for k_{m}: {x} {{ @{m}[k_{m}] }});")
        elif kind == 4:
            t = fresh(rng, "Type")
            f1, f2 = fresh(rng), fresh(rng)
            d1 = pick_dim(rng)
            d2 = pick_dim(rng)
            w = fresh(rng)
            r = fresh(rng)
            out.append(
                f"type {t} {{\n    {t}({f1}: {d1}, {f2}: {d2}),\n}}\n"
                f"node {w}: {t} = {t}({f1}: {qlit(rng, d1)}, {f2}: {qlit(rng, d2)});\n"
                f"node {r}: {d1} = @{w}.{f1};")
        elif kind == 5:
            d = pick_dim(rng)
            a = fresh(rng)
            out.append(f"param {a}: {d} = {qlit(rng, d)};\n"
                       f"assert {fresh(rng)} = @{a} == @{a};")
        elif kind == 6:
            a = fresh(rng)
            out.append(f"node {a}: Dimensionless = ln({flit(rng, 1.0, 50.0)}) "
                       f"+ exp({flit(rng, 0.0, 2.0)}) * cos({flit(rng)} rad);")
        elif kind == 7:
            a = fresh(rng)
            b = fresh(rng)
            out.append(
                f"node {a}: Complex<Dimensionless> = complex({flit(rng)}, {flit(rng)});\n"
                f"node {b}: Dimensionless = re(@{a}) + im(@{a});")
        elif kind == 8:
            dg = fresh(rng)
            pa = fresh(rng)
            no = fresh(rng)
            al = fresh(rng)
            d = pick_dim(rng)
            out.append(
                f"dag {dg} {{\n    param {pa}: {d};\n"
                f"    node {no}: {d} = @{pa} * 2.0;\n}}\n"
                f"include {dg}({pa}: {qlit(rng, d)}).{{ {no} as {al} }};")
        else:
            a = fresh(rng)
            d = pick_dim(rng)
            un = unit_of(rng, d)
            out.append(f"param {a}: {d} = {qlit(rng, d)};\n"
                       f"node {fresh(rng)}: {d} = @{a} -> {un};")
    return out


# ------------------------------------------------------------------ recipes
# Each recipe: fn(ctx) -> dict(probe=str decls, control=str|None, reason,
#                              confidence)

RECIPES = {}


def recipe(name):
    def reg(fn):
        RECIPES[name] = fn
        return fn
    return reg


# --- dimension & numeric-kind errors ---------------------------------------

@recipe("add_dim_mismatch")
def r_add_dim_mismatch(c):
    d1, d2 = c.dim2()
    n = c.v()
    op = c.rng.choice(["+", "-"])
    return dict(
        probe=f"node {n}: {d1} = {c.q(d1)} {op} {c.q(d2)};",
        control=f"node {n}: {d1} = {c.q(d1)} {op} {c.q(d1)};",
        reason=f"operands of `{op}` must share one dimension ({d1} vs {d2})",
        confidence="high")


@recipe("annotation_dim_mismatch")
def r_annotation_dim_mismatch(c):
    d1, d2 = c.dim2()
    n = c.v()
    return dict(
        probe=f"node {n}: {d1} = {c.q(d2)};",
        control=f"node {n}: {d1} = {c.q(d1)};",
        reason=f"declared {d1} but expression has dimension {d2}",
        confidence="high")


@recipe("param_annotation_dim_mismatch")
def r_param_annotation_dim_mismatch(c):
    d1, d2 = c.dim2()
    n = c.v()
    return dict(
        probe=f"param {n}: {d1} = {c.q(d2)};",
        control=f"param {n}: {d1} = {c.q(d1)};",
        reason=f"param declared {d1} but default has dimension {d2}",
        confidence="high")


@recipe("const_annotation_dim_mismatch")
def r_const_annotation_dim_mismatch(c):
    d1, d2 = c.dim2()
    n = c.v()
    return dict(
        probe=f"const node {n}: {d1} = {c.q(d2)};",
        control=f"const node {n}: {d1} = {c.q(d1)};",
        reason=f"const node declared {d1} but value has dimension {d2}",
        confidence="high")


@recipe("mul_result_dim_mismatch")
def r_mul_result_dim_mismatch(c):
    n = c.v()
    return dict(
        probe=f"node {n}: Length = {c.q('Length')} * {c.q('Length')};",
        control=f"node {n}: Area = {c.q('Length')} * {c.q('Length')};",
        reason="Length*Length is Area, annotation says Length",
        confidence="high")


@recipe("div_result_dim_mismatch")
def r_div_result_dim_mismatch(c):
    n = c.v()
    return dict(
        probe=f"node {n}: Time = {c.q('Length')} / {c.q('Time')};",
        control=f"node {n}: Velocity = {c.q('Length')} / {c.q('Time')};",
        reason="Length/Time is Velocity, annotation says Time",
        confidence="high")


@recipe("sqrt_result_mismatch")
def r_sqrt_result_mismatch(c):
    n = c.v()
    return dict(
        probe=f"node {n}: Area = sqrt({c.q('Area')});",
        control=f"node {n}: Length = sqrt({c.q('Area')});",
        reason="sqrt halves exponents: sqrt(Area)=Length, not Area",
        confidence="high")


@recipe("trig_requires_angle")
def r_trig_requires_angle(c):
    n = c.v()
    fn = c.rng.choice(["sin", "cos", "tan"])
    d = c.rng.choice(["Mass", "Length", "Time", "Dimensionless"])
    arg = c.f() if d == "Dimensionless" else c.q(d)
    return dict(
        probe=f"node {n}: Dimensionless = {fn}({arg});",
        control=f"node {n}: Dimensionless = {fn}({c.f()} rad);",
        reason=f"{fn} requires an Angle argument, got {d}",
        confidence="high")


@recipe("exp_requires_dimensionless")
def r_exp_requires_dimensionless(c):
    n = c.v()
    fn = c.rng.choice(["exp", "ln", "log10", "log2", "expm1", "sinh", "tanh"])
    d = pick_dim(c.rng, exclude=("Dimensionless",))
    return dict(
        probe=f"node {n}: Dimensionless = {fn}({c.q(d)});",
        control=f"node {n}: Dimensionless = {fn}({c.f(0.2, 3.0)});",
        reason=f"{fn} requires a Dimensionless argument, got {d}",
        confidence="high")


@recipe("rounding_requires_dimensionless")
def r_rounding_requires_dimensionless(c):
    n = c.v()
    fn = c.rng.choice(["floor", "ceil", "round", "trunc"])
    d = pick_dim(c.rng, exclude=("Dimensionless",))
    return dict(
        probe=f"node {n}: Dimensionless = {fn}({c.q(d)});",
        control=f"node {n}: Dimensionless = {fn}({c.f()});",
        reason=f"{fn} accepts only Dimensionless arguments (D-rule)",
        confidence="high")


@recipe("atan2_operand_mismatch")
def r_atan2(c):
    d1, d2 = c.dim2()
    n = c.v()
    return dict(
        probe=f"node {n}: Angle = atan2({c.q(d1)}, {c.q(d2)});",
        control=f"node {n}: Angle = atan2({c.q(d1)}, {c.q(d1)});",
        reason="atan2 needs both operands of one dimension",
        confidence="high")


@recipe("binary_fn_operand_mismatch")
def r_binary_fn(c):
    d1, d2 = c.dim2()
    n = c.v()
    fn = c.rng.choice(["least", "greatest", "hypot"])
    return dict(
        probe=f"node {n}: {d1} = {fn}({c.q(d1)}, {c.q(d2)});",
        control=f"node {n}: {d1} = {fn}({c.q(d1)}, {c.q(d1)});",
        reason=f"{fn} needs both operands of one dimension",
        confidence="high")


@recipe("clamp_bound_mismatch")
def r_clamp(c):
    d1, d2 = c.dim2()
    n = c.v()
    return dict(
        probe=f"node {n}: {d1} = clamp({c.q(d1)}, {c.q(d1, 0.01, 0.5)}, {c.q(d2)});",
        control=f"node {n}: {d1} = clamp({c.q(d1)}, {c.q(d1, 0.01, 0.5)}, {c.q(d1, 1000.0, 2000.0)});",
        reason="clamp requires all three operands of one dimension",
        confidence="high")


@recipe("int_float_mix")
def r_int_float_mix(c):
    n = c.v()
    op = c.rng.choice(["+", "-", "*", "/"])
    return dict(
        probe=f"node {n}: Dimensionless = {ilit(c.rng)} {op} {c.f()};",
        control=f"node {n}: Dimensionless = to_float({ilit(c.rng)}) {op} {c.f()};",
        reason="Int and quantity never mix without explicit to_float",
        confidence="high")


@recipe("int_annotation_float_expr")
def r_int_annotation(c):
    n = c.v()
    return dict(
        probe=f"node {n}: Int = {c.f()};",
        control=f"node {n}: Int = {ilit(c.rng)};",
        reason="Int annotation with a float (Dimensionless) literal",
        confidence="high")


@recipe("float_annotation_int_expr")
def r_float_annotation(c):
    n = c.v()
    return dict(
        probe=f"node {n}: Dimensionless = {ilit(c.rng)};",
        control=f"node {n}: Dimensionless = to_float({ilit(c.rng)});",
        reason="Dimensionless annotation with an Int literal (no implicit conversion)",
        confidence="high")


@recipe("int_literal_with_unit")
def r_int_literal_with_unit(c):
    n = c.v()
    d = pick_dim(c.rng, exclude=("Dimensionless",))
    un = c.u(d)
    return dict(
        probe=f"node {n}: {d} = {ilit(c.rng)} {un};",
        control=f"node {n}: {d} = {ilit(c.rng)}.0 {un};",
        reason="integer literals cannot carry units",
        confidence="high")


@recipe("bool_arith")
def r_bool_arith(c):
    n = c.v()
    e = c.rng.choice([
        "true + false", f"true + {c.f()}", f"{c.f()} * false", "-true",
        f"true - {ilit(c.rng)}"])
    return dict(
        probe=f"node {n}: Bool = {e};",
        control=f"node {n}: Bool = true && false;",
        reason=f"arithmetic on Bool operands: `{e}`",
        confidence="high")


@recipe("logic_on_non_bool")
def r_logic_non_bool(c):
    n = c.v()
    e = c.rng.choice([
        f"{c.f()} && true", f"true || {c.f()}", f"!{c.f()}",
        f"{c.q('Mass')} && {c.q('Mass')}"])
    return dict(
        probe=f"node {n}: Bool = {e};",
        control=f"node {n}: Bool = true || false;",
        reason=f"logical operator on non-Bool operand: `{e}`",
        confidence="high")


@recipe("if_cond_not_bool")
def r_if_cond(c):
    n = c.v()
    cond = c.rng.choice([c.f(), ilit(c.rng), c.q("Mass")])
    return dict(
        probe=f"node {n}: Dimensionless = if {cond} {{ {c.f()} }} else {{ {c.f()} }};",
        control=f"node {n}: Dimensionless = if {c.f()} > {c.f()} {{ {c.f()} }} else {{ {c.f()} }};",
        reason="if condition must be Bool",
        confidence="high")


@recipe("if_branch_mismatch")
def r_if_branch(c):
    d1, d2 = c.dim2()
    n = c.v()
    return dict(
        probe=(f"node {n}: {d1} = if {c.f()} > {c.f()} "
               f"{{ {c.q(d1)} }} else {{ {c.q(d2)} }};"),
        control=(f"node {n}: {d1} = if {c.f()} > {c.f()} "
                 f"{{ {c.q(d1)} }} else {{ {c.q(d1)} }};"),
        reason=f"if branches disagree: {d1} vs {d2}",
        confidence="high")


@recipe("cmp_dim_mismatch")
def r_cmp_dim(c):
    d1, d2 = c.dim2()
    n = c.v()
    op = c.rng.choice(["<", ">", "<=", ">=", "==", "!="])
    return dict(
        probe=f"node {n}: Bool = {c.q(d1)} {op} {c.q(d2)};",
        control=f"node {n}: Bool = {c.q(d1)} {op} {c.q(d1)};",
        reason=f"comparison `{op}` across dimensions {d1} vs {d2}",
        confidence="high")


@recipe("cmp_int_quantity")
def r_cmp_int_quantity(c):
    n = c.v()
    return dict(
        probe=f"node {n}: Bool = {ilit(c.rng)} < {c.f()};",
        control=f"node {n}: Bool = to_float({ilit(c.rng)}) < {c.f()};",
        reason="Int compared against a quantity without conversion",
        confidence="high")


@recipe("pow_decimal_exponent_dimensioned")
def r_pow_decimal(c):
    n = c.v()
    d = pick_dim(c.rng, exclude=("Dimensionless",))
    exp = c.rng.choice(["1.5", "2.0", "0.25"])
    return dict(
        probe=f"node {n}: {d}^2 = ({c.q(d)}) ^ 2.0;" if exp == "2.0" else
              f"node {n}: Dimensionless = (({c.q(d)}) ^ {exp}) / (({c.q(d)}) ^ {exp});",
        control=f"node {n}: {d}^2 = ({c.q(d)}) ^ 2;",
        reason="dimensioned base needs exact integer/rational exponent syntax (D020)",
        confidence="high")


@recipe("pow_runtime_exponent_dimensioned")
def r_pow_runtime_exp(c):
    n, p = c.v(), c.v()
    d = pick_dim(c.rng, exclude=("Dimensionless",))
    return dict(
        probe=(f"param {p}: Dimensionless = {c.f(1.0, 3.0)};\n"
               f"node {n}: {d} = ({c.q(d)}) ^ @{p};"),
        control=(f"param {p}: Dimensionless = {c.f(1.0, 3.0)};\n"
                 f"node {n}: Dimensionless = {c.f()} ^ @{p};"),
        reason="runtime exponent over a dimensioned base cannot type",
        confidence="high")


@recipe("pow_exponent_has_dimension")
def r_pow_dim_exp(c):
    n = c.v()
    d = pick_dim(c.rng, exclude=("Dimensionless",))
    return dict(
        probe=f"node {n}: Dimensionless = {c.f()} ^ ({c.q(d)});",
        control=f"node {n}: Dimensionless = {c.f()} ^ {c.f(0.5, 2.0)};",
        reason="every exponent must be dimensionless",
        confidence="high")


@recipe("int_pow_negative")
def r_int_pow_negative(c):
    n = c.v()
    return dict(
        probe=f"node {n}: Int = {ilit(c.rng, 2, 9)} ^ -{ilit(c.rng, 1, 3)};",
        control=f"node {n}: Int = {ilit(c.rng, 2, 9)} ^ {ilit(c.rng, 1, 3)};",
        reason="Int ^ Int requires a non-negative exponent",
        confidence="high")


@recipe("nonfinite_literal")
def r_nonfinite_literal(c):
    n = c.v()
    return dict(
        probe=f"node {n}: Dimensionless = 1.0e{c.rng.randint(309, 999)};",
        control=f"node {n}: Dimensionless = 1.0e{c.rng.randint(100, 300)};",
        reason="float literal overflows to infinity; literals must stay finite",
        confidence="high")


# --- name resolution --------------------------------------------------------

@recipe("unknown_ref")
def r_unknown_ref(c):
    n = c.v()
    ghost = c.v() + "_ghost"
    return dict(
        probe=f"node {n}: Dimensionless = @{ghost} + {c.f()};",
        control=(f"param {ghost}: Dimensionless = {c.f()};\n"
                 f"node {n}: Dimensionless = @{ghost} + {c.f()};"),
        reason=f"reference to undeclared @{ghost}",
        confidence="high")


@recipe("unknown_unit")
def r_unknown_unit(c):
    n = c.v()
    ghost = fresh(c.rng)
    return dict(
        probe=f"node {n}: Length = {c.f()} {ghost};",
        control=f"node {n}: Length = {c.f()} m;",
        reason=f"unknown unit `{ghost}` (D003)",
        confidence="high")


@recipe("unknown_type_annotation")
def r_unknown_type(c):
    n = c.v()
    ghost = c.T()
    return dict(
        probe=f"node {n}: {ghost} = {c.f()};",
        control=f"node {n}: Dimensionless = {c.f()};",
        reason=f"unknown type/dimension `{ghost}` in annotation",
        confidence="high")


@recipe("unknown_function")
def r_unknown_function(c):
    n = c.v()
    ghost = fresh(c.rng)
    return dict(
        probe=f"node {n}: Dimensionless = {ghost}({c.f()});",
        control=f"node {n}: Dimensionless = abs({c.f()});",
        reason=f"call to unknown function `{ghost}`",
        confidence="high")


@recipe("duplicate_value_decl")
def r_duplicate_value(c):
    n = c.v()
    k1 = c.rng.choice(["param", "node", "const node"])
    k2 = c.rng.choice(["param", "node", "const node"])
    m = c.v()
    return dict(
        probe=(f"{k1} {n}: Dimensionless = {c.f()};\n"
               f"{k2} {n}: Dimensionless = {c.f()};"),
        control=(f"{k1} {n}: Dimensionless = {c.f()};\n"
                 f"{k2} {m}: Dimensionless = {c.f()};"),
        reason=f"name `{n}` declared twice in the value universe",
        confidence="high")


@recipe("cross_universe_collision")
def r_cross_universe(c):
    t = c.T()
    other = c.T()
    pair = c.rng.choice([
        (f"type {t} {{ {t}Mk }}", f"index {t} = {{ A{t}, B{t} }};"),
        (f"dim {t} = Length * Mass;", f"type {t} {{ {t}Mk }}"),
        (f"dim {t} = Length / Time;", f"index {t} = {{ A{t}, B{t} }};"),
    ])
    return dict(
        probe=pair[0] + "\n" + pair[1],
        control=pair[0] + "\n" + pair[1].replace(t, other, 1),
        reason=f"`{t}` declared in two exclusive universes",
        confidence="high")


@recipe("const_references_graph")
def r_const_refs_graph(c):
    n, p = c.v(), c.v()
    return dict(
        probe=(f"param {p}: Dimensionless = {c.f()};\n"
               f"const node {n}: Dimensionless = @{p} * 2.0;"),
        control=(f"param {p}: Dimensionless = {c.f()};\n"
                 f"const node {n}: Dimensionless = {c.f()} * 2.0;"),
        reason="const node bodies cannot reference @ graph values",
        confidence="high")


@recipe("param_default_references_graph")
def r_param_default_ref(c):
    n, p = c.v(), c.v()
    return dict(
        probe=(f"param {p}: Dimensionless = {c.f()};\n"
               f"param {n}: Dimensionless = @{p} + 1.0;"),
        control=(f"param {p}: Dimensionless = {c.f()};\n"
                 f"param {n}: Dimensionless = {c.f()} + 1.0;"),
        reason="param defaults cannot reference @ graph values",
        confidence="high")


@recipe("assert_referenced_with_at")
def r_assert_at(c):
    a, n, p = c.v(), c.v(), c.v()
    return dict(
        probe=(f"param {p}: Dimensionless = {c.f()};\n"
               f"assert {a} = @{p} > 0.0;\n"
               f"node {n}: Bool = @{a};"),
        control=(f"param {p}: Dimensionless = {c.f()};\n"
                 f"assert {a} = @{p} > 0.0;\n"
                 f"node {n}: Bool = @{p} > 0.0;"),
        reason="assert cannot be referenced with @ (A003)",
        confidence="high")


@recipe("cycle_direct")
def r_cycle(c):
    a, b = c.v(), c.v()
    style = c.rng.random()
    if style < 0.4:
        probe = f"node {a}: Dimensionless = @{a} + 1.0;"
    elif style < 0.8:
        probe = (f"node {a}: Dimensionless = @{b} + 1.0;\n"
                 f"node {b}: Dimensionless = @{a} * 2.0;")
    else:
        z = c.v()
        probe = (f"node {a}: Dimensionless = @{b} + 1.0;\n"
                 f"node {b}: Dimensionless = @{z} * 2.0;\n"
                 f"node {z}: Dimensionless = @{a} - 1.0;")
    return dict(
        probe=probe,
        control=(f"node {a}: Dimensionless = {c.f()} + 1.0;\n"
                 f"node {b}: Dimensionless = @{a} * 2.0;"),
        reason="dependency cycle between nodes",
        confidence="high")


@recipe("const_cycle")
def r_const_cycle(c):
    a, b = c.v(), c.v()
    return dict(
        probe=(f"const node {a}: Dimensionless = {c.f()};\n"
               f"const unit {b}_u: Length = (1.0 + @{a} * 0.0) m;"),
        control=None,  # const-unit @ refs are themselves invalid; simpler pair below
        reason="const unit scale cannot contain @ references",
        confidence="high")


@recipe("pub_on_param")
def r_pub_param(c):
    n = c.v()
    return dict(
        probe=f"pub param {n}: Length = {c.q('Length')};",
        control=f"param {n}: Length = {c.q('Length')};",
        reason="params never take pub (declaration kind supplies the role)",
        confidence="high")


@recipe("assumes_unknown_assert")
def r_assumes_unknown(c):
    n = c.v()
    ghost = c.v()
    a, p = c.v(), c.v()
    return dict(
        probe=(f"#[assumes({ghost})]\n"
               f"node {n}: Dimensionless = {c.f()};"),
        control=(f"param {p}: Dimensionless = {c.f()};\n"
                 f"assert {a} = @{p} > 0.0;\n"
                 f"#[assumes({a})]\n"
                 f"node {n}: Dimensionless = {c.f()};"),
        reason=f"#[assumes] names unknown assertion `{ghost}` (A005)",
        confidence="high")


@recipe("assumes_on_const")
def r_assumes_on_const(c):
    n, a, p = c.v(), c.v(), c.v()
    return dict(
        probe=(f"param {p}: Dimensionless = {c.f()};\n"
               f"assert {a} = @{p} > 0.0;\n"
               f"#[assumes({a})]\n"
               f"const node {n}: Dimensionless = {c.f()};"),
        control=(f"param {p}: Dimensionless = {c.f()};\n"
                 f"assert {a} = @{p} > 0.0;\n"
                 f"#[assumes({a})]\n"
                 f"node {n}: Dimensionless = {c.f()};"),
        reason="#[assumes] on const node (A006)",
        confidence="high")


@recipe("unknown_attribute")
def r_unknown_attr(c):
    n = c.v()
    ghost = fresh(c.rng)
    return dict(
        probe=f"#[{ghost}]\nnode {n}: Dimensionless = {c.f()};",
        control=f"node {n}: Dimensionless = {c.f()};",
        reason=f"unknown attribute #[{ghost}] (A007)",
        confidence="high")


@recipe("expected_fail_on_node")
def r_ef_on_node(c):
    n = c.v()
    kind = c.rng.choice(["node", "param", "const node"])
    return dict(
        probe=f"#[expected_fail]\n{kind} {n}: Dimensionless = {c.f()};",
        control=f"{kind} {n}: Dimensionless = {c.f()};",
        reason="#[expected_fail] on non-assert declaration (A008)",
        confidence="high")


@recipe("expected_fail_args_on_unindexed")
def r_ef_args_unindexed(c):
    a, p = c.v(), c.v()
    x = c.T()
    la, lb = c.T(), c.T()
    return dict(
        probe=(f"index {x} = {{ {la}, {lb} }};\n"
               f"param {p}: Dimensionless = {c.f()};\n"
               f"#[expected_fail({x}.{la})]\n"
               f"assert {a} = @{p} > 0.0;"),
        control=(f"index {x} = {{ {la}, {lb} }};\n"
                 f"param {p}: Dimensionless = {c.f()};\n"
                 f"#[expected_fail]\n"
                 f"assert {a} = @{p} < 0.0;"),
        reason="variant keys on an unindexed assertion (A010)",
        confidence="high")


@recipe("expected_fail_missing_keys_on_indexed")
def r_ef_missing_keys(c):
    a, p = c.v(), c.v()
    x = c.T()
    la, lb = c.T(), c.T()
    return dict(
        probe=(f"pub index {x} = {{ {la}, {lb} }};\n"
               f"param {p}: Dimensionless[{x}] = {{ {x}.{la}: {c.f()}, {x}.{lb}: {c.f()} }};\n"
               f"#[expected_fail]\n"
               f"assert {a} = for k: {x} {{ @{p}[k] > 0.0 }};"),
        control=(f"pub index {x} = {{ {la}, {lb} }};\n"
                 f"param {p}: Dimensionless[{x}] = {{ {x}.{la}: {c.f()}, {x}.{lb}: {c.f()} }};\n"
                 f"#[expected_fail({x}.{la})]\n"
                 f"assert {a} = for k: {x} {{ @{p}[k] > 0.0 }};"),
        reason="#[expected_fail] without keys on an indexed assertion (A011)",
        confidence="high")


@recipe("tolerance_negative")
def r_tol_negative(c):
    a, p = c.v(), c.v()
    d = pick_dim(c.rng)
    return dict(
        probe=(f"param {p}: {d} = {c.q(d)};\n"
               f"assert {a} = @{p} ~= {c.q(d)} +/- -{c.q(d, 0.01, 1.0)};"),
        control=(f"param {p}: {d} = {c.q(d)};\n"
                 f"assert {a} = @{p} ~= {c.q(d)} +/- {c.q(d, 0.01, 1.0)};"),
        reason="literal negative tolerance (A015)",
        confidence="high")


@recipe("tolerance_dim_mismatch")
def r_tol_dim(c):
    a, p = c.v(), c.v()
    d1, d2 = c.dim2()
    return dict(
        probe=(f"param {p}: {d1} = {c.q(d1)};\n"
               f"assert {a} = @{p} ~= {c.q(d1)} +/- {c.q(d2, 0.01, 1.0)};"),
        control=(f"param {p}: {d1} = {c.q(d1)};\n"
                 f"assert {a} = @{p} ~= {c.q(d1)} +/- {c.q(d1, 0.01, 1.0)};"),
        reason=f"tolerance dimension {d2} differs from actual {d1}",
        confidence="high")


@recipe("assert_body_not_bool")
def r_assert_not_bool(c):
    a = c.v()
    d = pick_dim(c.rng)
    return dict(
        probe=f"assert {a} = {c.q(d)};",
        control=f"assert {a} = {c.q(d)} == {c.q(d)};",
        reason="assert body must evaluate to Bool (A004)",
        confidence="high")


# --- units ------------------------------------------------------------------

@recipe("unit_rhs_dim_mismatch")
def r_unit_rhs(c):
    u = fresh(c.rng)
    while True:
        d1, d2 = c.dim2()
        if "Temperature" not in (d1, d2):
            break
    return dict(
        probe=f"const unit {u}: {d1} = {c.f(0.5, 100.0)} {c.u(d2)};",
        control=f"const unit {u}: {d1} = {c.f(0.5, 100.0)} {c.u(d1)};",
        reason=f"unit declared {d1} but body has dimension {d2}",
        confidence="high")


@recipe("unit_nonpositive_scale")
def r_unit_scale(c):
    u = fresh(c.rng)
    d = pick_dim(c.rng, exclude=("Temperature",))
    scale = c.rng.choice(["0.0", f"-{c.f(0.1, 10.0)}"])
    return dict(
        probe=f"const unit {u}: {d} = {scale} {c.u(d)};",
        control=f"const unit {u}: {d} = {c.f(0.5, 100.0)} {c.u(d)};",
        reason="unit scale must be positive and finite",
        confidence="high")


@recipe("bare_temperature_unit")
def r_temp_unit(c):
    u = fresh(c.rng)
    return dict(
        probe=f"const unit {u}: Temperature = {c.f(0.5, 2.0)} K;",
        control=f"const unit {u}: Temperature / Time = {c.f(0.5, 2.0)} K/s;",
        reason="user unit definitions on bare Temperature are rejected (D014)",
        confidence="high")


@recipe("const_unit_with_at")
def r_const_unit_at(c):
    u, p = fresh(c.rng), c.v()
    return dict(
        probe=(f"param {p}: Dimensionless = {c.f(1.0, 2.0)};\n"
               f"const unit {u}: Length = (@{p}) m;"),
        control=(f"param {p}: Dimensionless = {c.f(1.0, 2.0)};\n"
                 f"unit {u}: Length = (@{p}) m;"),
        reason="a const unit scale cannot contain @ references",
        confidence="high")


@recipe("dynamic_unit_scale_not_dimensionless")
def r_dyn_unit_scale(c):
    u, p = fresh(c.rng), c.v()
    d = pick_dim(c.rng, exclude=("Dimensionless",))
    return dict(
        probe=(f"param {p}: {d} = {c.q(d)};\n"
               f"unit {u}: Length = (@{p}) m;"),
        control=(f"param {p}: Dimensionless = {c.f(1.0, 3.0)};\n"
                 f"unit {u}: Length = (@{p}) m;"),
        reason=f"dynamic unit scale must be Dimensionless, got {d} (D032)",
        confidence="high")


@recipe("duplicate_unit")
def r_duplicate_unit(c):
    u = fresh(c.rng)
    return dict(
        probe=(f"const unit {u}: Length = {c.f(2.0, 9.0)} m;\n"
               f"const unit {u}: Length = {c.f(20.0, 90.0)} m;"),
        control=(f"const unit {u}: Length = {c.f(2.0, 9.0)} m;\n"
                 f"const unit {u}x: Length = {c.f(20.0, 90.0)} m;"),
        reason=f"unit `{u}` declared twice",
        confidence="high")


@recipe("redefine_prelude_unit")
def r_redefine_prelude_unit(c):
    un = c.rng.choice(["km", "kg", "min", "kN", "Hz"])
    dim = {"km": "Length", "kg": "Mass", "min": "Time", "kN": "Force",
           "Hz": "Frequency"}[un]
    base = {"km": "m", "kg": "g", "min": "s", "kN": "N", "Hz": "Hz"}[un]
    u2 = fresh(c.rng)
    return dict(
        probe=f"const unit {un}: {dim} = {c.f(2.0, 500.0)} {base};",
        control=f"const unit {u2}: {dim} = {c.f(2.0, 500.0)} {base};",
        reason=f"redefining prelude unit `{un}` with a different scale",
        confidence="med")


@recipe("unit_cycle")
def r_unit_cycle(c):
    a, b = fresh(c.rng), fresh(c.rng)
    return dict(
        probe=(f"const unit {a}: Length = 2.0 {b};\n"
               f"const unit {b}: Length = 3.0 {a};"),
        control=(f"const unit {a}: Length = 2.0 m;\n"
                 f"const unit {b}: Length = 3.0 {a};"),
        reason="unit definitions form a cycle",
        confidence="high")


@recipe("base_unit_second_canonical")
def r_base_unit_second(c):
    d = c.T()
    u1, u2 = fresh(c.rng), fresh(c.rng)
    n = c.v()
    return dict(
        probe=(f"base dim {d};\n"
               f"base unit {u1}: {d};\n"
               f"base unit {u2}: {d};\n"
               f"node {n}: {d} = {c.f()} {u1};"),
        control=(f"base dim {d};\n"
                 f"base unit {u1}: {d};\n"
                 f"const unit {u2}: {d} = {c.f(2.0, 9.0)} {u1};\n"
                 f"node {n}: {d} = {c.f()} {u1};"),
        reason="two base units for one dimension silently share scale 1.0 "
               "(two distinct canonical units cannot both be canonical)",
        confidence="med")


@recipe("base_unit_on_prelude_dim")
def r_base_unit_prelude(c):
    u = fresh(c.rng)
    d = c.rng.choice(BASE_DIMS)
    n = c.v()
    return dict(
        probe=(f"base unit {u}: {d};\n"
               f"node {n}: {d} = {c.f()} {u};"),
        control=(f"const unit {u}: {d} = {c.f(2.0, 9.0)} {DIMS[d][1][0]};\n"
                 f"node {n}: {d} = {c.f()} {u};"),
        reason=f"base unit on prelude dimension {d} silently aliases its SI base "
               f"unit (scale 1.0) — e.g. a new 'furlong' becomes exactly 1 m",
        confidence="med")


@recipe("base_unit_on_derived_dim")
def r_base_unit_derived(c):
    u = fresh(c.rng)
    d = c.rng.choice(["Velocity", "Force", "Energy", "Pressure"])
    n = c.v()
    return dict(
        probe=(f"base unit {u}: {d};\n"
               f"node {n}: {d} = {c.f()} {u};"),
        control=(f"const unit {u}: {d} = {c.f(2.0, 9.0)} {DIMS[d][1][0]};\n"
                 f"node {n}: {d} = {c.f()} {u};"),
        reason=f"base unit on derived dimension {d}: not a user-defined base "
               "dimension, silently aliases the SI-coherent scale",
        confidence="med")


@recipe("base_unit_on_temperature")
def r_base_unit_temp(c):
    u = fresh(c.rng)
    n = c.v()
    return dict(
        probe=(f"base unit {u}: Temperature;\n"
               f"node {n}: Temperature = {c.f()} {u};"),
        control=f"node {n}: Temperature = {c.f()} K;",
        reason="base unit on Temperature bypasses the D014 affine-unit guard",
        confidence="med")


@recipe("dim_self_reference")
def r_dim_self(c):
    d = c.T()
    return dict(
        probe=f"dim {d} = {d} * Length;",
        control=f"dim {d} = Time * Length;",
        reason="dimension definition references itself",
        confidence="high")


@recipe("redefine_prelude_dim")
def r_redefine_prelude_dim(c):
    d2 = c.T()
    target = c.rng.choice(["Velocity", "Force", "Energy"])
    return dict(
        probe=f"dim {target} = Length * Mass;",
        control=f"dim {d2} = Length * Mass;",
        reason=f"redefining prelude dimension {target} with a different formula",
        confidence="med")


@recipe("dim_zero_exponent")
def r_dim_zero_exp(c):
    d = c.T()
    return dict(
        probe=f"dim {d} = Length^0;",
        control=f"dim {d} = Length^2;",
        reason="zero exponents are omitted from dimension declarations",
        confidence="high")


# --- conversion -------------------------------------------------------------

@recipe("convert_dim_mismatch")
def r_convert_dim(c):
    n, p = c.v(), c.v()
    d1, d2 = c.dim2()
    return dict(
        probe=(f"param {p}: {d1} = {c.q(d1)};\n"
               f"node {n}: {d1} = @{p} -> {c.u(d2)};"),
        control=(f"param {p}: {d1} = {c.q(d1)};\n"
                 f"node {n}: {d1} = @{p} -> {c.u(d1)};"),
        reason=f"conversion target unit of {d2} on a {d1} value",
        confidence="high")


@recipe("convert_chained_paren")
def r_convert_chain(c):
    n, p = c.v(), c.v()
    return dict(
        probe=(f"param {p}: Length = {c.q('Length')};\n"
               f"node {n}: Length = (@{p} -> km) -> m;"),
        control=(f"param {p}: Length = {c.q('Length')};\n"
                 f"node {n}: Length = @{p} -> m;"),
        reason="chained conversion (D012)",
        confidence="high")


@recipe("convert_in_arithmetic")
def r_convert_arith(c):
    n, p = c.v(), c.v()
    return dict(
        probe=(f"param {p}: Length = {c.q('Length')};\n"
               f"node {n}: Length = (@{p} -> km) + {c.q('Length')};"),
        control=(f"param {p}: Length = {c.q('Length')};\n"
                 f"node {n}: Length = (@{p} + {c.q('Length')}) -> km;"),
        reason="conversion buried in an arithmetic operand (D013)",
        confidence="high")


@recipe("convert_in_fn_arg")
def r_convert_fnarg(c):
    n, p = c.v(), c.v()
    return dict(
        probe=(f"param {p}: Area = {c.q('Area')};\n"
               f"node {n}: Length = sqrt(@{p} -> m^2);"),
        control=(f"param {p}: Area = {c.q('Area')};\n"
                 f"node {n}: Length = sqrt(@{p});"),
        reason="conversion inside a function argument (D013)",
        confidence="high")


@recipe("convert_in_condition")
def r_convert_cond(c):
    n, p = c.v(), c.v()
    return dict(
        probe=(f"param {p}: Length = {c.q('Length')};\n"
               f"node {n}: Bool = (@{p} -> km) > {c.q('Length')};"),
        control=(f"param {p}: Length = {c.q('Length')};\n"
                 f"node {n}: Bool = @{p} > {c.q('Length')};"),
        reason="conversion in a comparison operand (D013)",
        confidence="high")


@recipe("convert_int_source")
def r_convert_int(c):
    n = c.v()
    return dict(
        probe=f"node {n}: Int = {ilit(c.rng)} -> m;",
        control=f"node {n}: Int = {ilit(c.rng)};",
        reason="conversion applied to an Int (no dimension to convert)",
        confidence="high")


# --- indexes ----------------------------------------------------------------

def _mkindex(c, k=3):
    x = c.T()
    labels = [c.T() for _ in range(k)]
    decl = f"index {x} = {{ {', '.join(labels)} }};"
    return x, labels, decl


@recipe("bare_nat_axis")
def r_bare_nat_axis(c):
    n = c.v()
    k = c.rng.randint(2, 5)
    return dict(
        probe=f"node {n}: Dimensionless[{k}] = for i: Fin({k}) {{ {c.f()} }};",
        control=f"node {n}: Dimensionless[Fin({k})] = for i: Fin({k}) {{ {c.f()} }};",
        reason="a bare Nat is not an Index; D[3] must be D[Fin(3)]",
        confidence="high")


@recipe("fin_zero")
def r_fin_zero(c):
    n = c.v()
    return dict(
        probe=f"node {n}: Dimensionless[Fin(0)] = for i: Fin(0) {{ {c.f()} }};",
        control=f"node {n}: Dimensionless[Fin(1)] = for i: Fin(1) {{ {c.f()} }};",
        reason="Fin(0) is invalid: every axis is finite and non-empty",
        confidence="high")


@recipe("map_missing_label")
def r_map_missing(c):
    x, ls, decl = _mkindex(c, 3)
    n = c.v()
    d = pick_dim(c.rng)
    entries = [f"{x}.{l}: {c.q(d)}" for l in ls]
    return dict(
        probe=f"{decl}\nnode {n}: {d}[{x}] = {{ {', '.join(entries[:2])} }};",
        control=f"{decl}\nnode {n}: {d}[{x}] = {{ {', '.join(entries)} }};",
        reason=f"map literal missing label {x}.{ls[2]} (must be total)",
        confidence="high")


@recipe("map_duplicate_label")
def r_map_dup(c):
    x, ls, decl = _mkindex(c, 2)
    n = c.v()
    d = pick_dim(c.rng)
    return dict(
        probe=(f"{decl}\nnode {n}: {d}[{x}] = {{ {x}.{ls[0]}: {c.q(d)}, "
               f"{x}.{ls[0]}: {c.q(d)}, {x}.{ls[1]}: {c.q(d)} }};"),
        control=(f"{decl}\nnode {n}: {d}[{x}] = {{ {x}.{ls[0]}: {c.q(d)}, "
                 f"{x}.{ls[1]}: {c.q(d)} }};"),
        reason="duplicate map key",
        confidence="high")


@recipe("map_foreign_label")
def r_map_foreign(c):
    x, ls, decl = _mkindex(c, 2)
    y, ms, decl2 = _mkindex(c, 2)
    n = c.v()
    d = pick_dim(c.rng)
    return dict(
        probe=(f"{decl}\n{decl2}\nnode {n}: {d}[{x}] = {{ {x}.{ls[0]}: {c.q(d)}, "
               f"{y}.{ms[1]}: {c.q(d)} }};"),
        control=(f"{decl}\n{decl2}\nnode {n}: {d}[{x}] = {{ {x}.{ls[0]}: {c.q(d)}, "
                 f"{x}.{ls[1]}: {c.q(d)} }};"),
        reason=f"map key from foreign index {y}",
        confidence="high")


@recipe("map_value_dim_mismatch")
def r_map_val_dim(c):
    x, ls, decl = _mkindex(c, 2)
    n = c.v()
    d1, d2 = c.dim2()
    return dict(
        probe=(f"{decl}\nnode {n}: {d1}[{x}] = {{ {x}.{ls[0]}: {c.q(d1)}, "
               f"{x}.{ls[1]}: {c.q(d2)} }};"),
        control=(f"{decl}\nnode {n}: {d1}[{x}] = {{ {x}.{ls[0]}: {c.q(d1)}, "
                 f"{x}.{ls[1]}: {c.q(d1)} }};"),
        reason=f"map entry dimension {d2} disagrees with {d1}",
        confidence="high")


@recipe("empty_index")
def r_empty_index(c):
    x = c.T()
    return dict(
        probe=f"index {x} = {{ }};",
        control=f"index {x} = {{ {c.T()} }};",
        reason="empty index: axes must be non-empty",
        confidence="high")


@recipe("duplicate_label_in_index")
def r_dup_label(c):
    x = c.T()
    a, b = c.T(), c.T()
    return dict(
        probe=f"index {x} = {{ {a}, {a}, {b} }};",
        control=f"index {x} = {{ {a}, {b} }};",
        reason="duplicate label inside one index declaration",
        confidence="high")


@recipe("partial_indexing")
def r_partial_index(c):
    x, ls, decl = _mkindex(c, 2)
    y, ms, decl2 = _mkindex(c, 2)
    n, m = c.v(), c.v()
    d = pick_dim(c.rng)
    mk = (f"node {m}: {d}[{x}, {y}] = for a: {x}, b: {y} {{ {c.q(d)} }};")
    return dict(
        probe=f"{decl}\n{decl2}\n{mk}\nnode {n}: {d} = @{m}[{x}.{ls[0]}];",
        control=f"{decl}\n{decl2}\n{mk}\nnode {n}: {d} = @{m}[{x}.{ls[0]}, {y}.{ms[0]}];",
        reason="partial indexing: all axes must be supplied",
        confidence="high")


@recipe("index_foreign_key")
def r_index_foreign_key(c):
    x, ls, decl = _mkindex(c, 2)
    y, ms, decl2 = _mkindex(c, 2)
    n, m = c.v(), c.v()
    d = pick_dim(c.rng)
    mk = f"node {m}: {d}[{x}] = for a: {x} {{ {c.q(d)} }};"
    return dict(
        probe=f"{decl}\n{decl2}\n{mk}\nnode {n}: {d} = @{m}[{y}.{ms[0]}];",
        control=f"{decl}\n{decl2}\n{mk}\nnode {n}: {d} = @{m}[{x}.{ls[0]}];",
        reason=f"indexing a {x}-axis value with a {y} key",
        confidence="high")


@recipe("indexed_cmp_broadcast")
def r_indexed_cmp(c):
    x, ls, decl = _mkindex(c, 2)
    n, m = c.v(), c.v()
    d = pick_dim(c.rng)
    mk = f"node {m}: {d}[{x}] = for a: {x} {{ {c.q(d)} }};"
    op = c.rng.choice(["<", ">", "<=", ">=", "==", "!="])
    return dict(
        probe=f"{decl}\n{mk}\nnode {n}: Bool = @{m} {op} {c.q(d)};",
        control=(f"{decl}\n{mk}\nnode {n}: Bool[{x}] = for a: {x} "
                 f"{{ @{m}[a] {op} {c.q(d)} }};"),
        reason=f"comparison `{op}` with an indexed operand (D019)",
        confidence="high")


@recipe("indexed_arith_broadcast")
def r_indexed_arith(c):
    x, ls, decl = _mkindex(c, 2)
    n, m = c.v(), c.v()
    d = pick_dim(c.rng)
    mk = f"node {m}: {d}[{x}] = for a: {x} {{ {c.q(d)} }};"
    op = c.rng.choice(["+", "-", "*"])
    return dict(
        probe=f"{decl}\n{mk}\nnode {n}: {d}[{x}] = @{m} {op} @{m};",
        control=(f"{decl}\n{mk}\nnode {n}: {d}[{x}] = for a: {x} "
                 f"{{ @{m}[a] {op if op != '*' else '+'} @{m}[a] }};"),
        reason="arithmetic never broadcasts over indexed values",
        confidence="high")


@recipe("agg_multi_axis")
def r_agg_multi(c):
    x, _, decl = _mkindex(c, 2)
    y, _, decl2 = _mkindex(c, 2)
    n, m = c.v(), c.v()
    fn = c.rng.choice(["sum", "maximum", "minimum", "mean", "rss"])
    mk = f"node {m}: Mass[{x}, {y}] = for a: {x}, b: {y} {{ {c.q('Mass')} }};"
    return dict(
        probe=f"{decl}\n{decl2}\n{mk}\nnode {n}: Mass = {fn}(@{m});",
        control=(f"{decl}\n{decl2}\n{mk}\nnode {n}: Mass = {fn}(for a: {x} "
                 f"{{ {fn}(for b: {y} {{ @{m}[a, b] }}) }});"),
        reason=f"{fn} reduces exactly one axis; multi-axis direct aggregation is D021",
        confidence="high")


@recipe("agg_scalar_arg")
def r_agg_scalar(c):
    n = c.v()
    fn = c.rng.choice(["sum", "maximum", "minimum", "mean", "count"])
    body = c.f()
    return dict(
        probe=(f"node {n}: {'Int' if fn == 'count' else 'Dimensionless'} = "
               f"{fn}({body});"),
        control=None,
        reason=f"{fn} requires an indexed argument, got a scalar",
        confidence="high")


@recipe("static_key_out_of_range")
def r_key_oob(c):
    n, m = c.v(), c.v()
    k = c.rng.randint(2, 5)
    mk = f"node {m}: Dimensionless[Fin({k})] = for i: Fin({k}) {{ {c.f()} }};"
    return dict(
        probe=f"{mk}\nnode {n}: Dimensionless = @{m}[key(Fin({k}), {k + c.rng.randint(0, 3)})];",
        control=f"{mk}\nnode {n}: Dimensionless = @{m}[key(Fin({k}), {k - 1})];",
        reason=f"key(Fin({k}), >= {k}) fails the compile-time range check",
        confidence="high")


@recipe("static_int_position_oob")
def r_static_int_oob(c):
    n, m = c.v(), c.v()
    k = c.rng.randint(2, 5)
    mk = f"node {m}: Dimensionless[Fin({k})] = for i: Fin({k}) {{ {c.f()} }};"
    return dict(
        probe=f"{mk}\nnode {n}: Dimensionless = @{m}[{k + c.rng.randint(0, 4)}];",
        control=f"{mk}\nnode {n}: Dimensionless = @{m}[{k - 1}];",
        reason="static Int position beyond the Fin axis bound",
        confidence="high")


@recipe("to_int_on_named_key")
def r_to_int_named(c):
    x, ls, decl = _mkindex(c, 2)
    n = c.v()
    return dict(
        probe=f"{decl}\nnode {n}: Int = to_int({x}.{ls[0]});",
        control=(f"{decl}\nnode {n}: Int = to_int(key(Fin(3), 1));"),
        reason="to_int extracts Fin keys only; named keys are opaque",
        confidence="high")


@recipe("coord_on_named_key")
def r_coord_named(c):
    x, ls, decl = _mkindex(c, 2)
    n = c.v()
    ci = c.T()
    return dict(
        probe=f"{decl}\nnode {n}: Time = coord({x}.{ls[0]});",
        control=(f"index {ci} = linspace(0.0 s, 9.0 s, points: 4);\n"
                 f"node {n}: Time = sum(for t: {ci} {{ coord(t) }});"),
        reason="coord extracts coordinate keys only; named keys are opaque",
        confidence="high")


@recipe("key_cross_axis_equality")
def r_key_cross_eq(c):
    x, ls, decl = _mkindex(c, 2)
    y, ms, decl2 = _mkindex(c, 2)
    n = c.v()
    return dict(
        probe=f"{decl}\n{decl2}\nnode {n}: Bool = {x}.{ls[0]} == {y}.{ms[0]};",
        control=f"{decl}\n{decl2}\nnode {n}: Bool = {x}.{ls[0]} == {x}.{ls[1]};",
        reason="key equality requires the same axis",
        confidence="high")


@recipe("key_ordering")
def r_key_ordering(c):
    x, ls, decl = _mkindex(c, 2)
    n = c.v()
    op = c.rng.choice(["<", ">", "<=", ">="])
    return dict(
        probe=f"{decl}\nnode {n}: Bool = {x}.{ls[0]} {op} {x}.{ls[1]};",
        control=f"{decl}\nnode {n}: Bool = {x}.{ls[0]} != {x}.{ls[1]};",
        reason="keys are unordered",
        confidence="high")


@recipe("match_key_nonexhaustive")
def r_match_key_nonex(c):
    x, ls, decl = _mkindex(c, 3)
    n, m = c.v(), c.v()
    mk = (f"node {m}: Dimensionless[{x}] = for a: {x} {{ match a {{ "
          + ", ".join(f"{x}.{l} => {c.f()}" for l in ls) + " } };")
    probe_match = (f"node {n}: Dimensionless[{x}] = for a: {x} {{ match a {{ "
                   + ", ".join(f"{x}.{l} => {c.f()}" for l in ls[:2]) + " } };")
    return dict(
        probe=f"{decl}\n{probe_match}",
        control=f"{decl}\n{mk}",
        reason=f"match over Key<{x}> missing label {ls[2]}",
        confidence="high")


@recipe("match_key_duplicate_arm")
def r_match_key_dup(c):
    x, ls, decl = _mkindex(c, 2)
    n = c.v()
    dup = (f"node {n}: Dimensionless[{x}] = for a: {x} {{ match a {{ "
           f"{x}.{ls[0]} => {c.f()}, {x}.{ls[0]} => {c.f()}, "
           f"{x}.{ls[1]} => {c.f()} }} }};")
    ok = (f"node {n}: Dimensionless[{x}] = for a: {x} {{ match a {{ "
          f"{x}.{ls[0]} => {c.f()}, {x}.{ls[1]} => {c.f()} }} }};")
    return dict(probe=f"{decl}\n{dup}", control=f"{decl}\n{ok}",
                reason="duplicate match arm for one label",
                confidence="high")


@recipe("for_body_axis_mismatch")
def r_for_axis_mismatch(c):
    x, _, decl = _mkindex(c, 2)
    y, _, decl2 = _mkindex(c, 2)
    n, m = c.v(), c.v()
    d = pick_dim(c.rng)
    mk = f"node {m}: {d}[{y}] = for b: {y} {{ {c.q(d)} }};"
    return dict(
        probe=f"{decl}\n{decl2}\n{mk}\nnode {n}: {d}[{x}] = for a: {x} {{ @{m}[a] }};",
        control=f"{decl}\n{decl2}\n{mk}\nnode {n}: {d}[{y}] = for b: {y} {{ @{m}[b] }};",
        reason=f"loop key of {x} used to index a {y}-axis value",
        confidence="high")


@recipe("for_annotation_axis_mismatch")
def r_for_annot_mismatch(c):
    x, _, decl = _mkindex(c, 2)
    y, _, decl2 = _mkindex(c, 2)
    n = c.v()
    d = pick_dim(c.rng)
    return dict(
        probe=f"{decl}\n{decl2}\nnode {n}: {d}[{x}] = for b: {y} {{ {c.q(d)} }};",
        control=f"{decl}\n{decl2}\nnode {n}: {d}[{y}] = for b: {y} {{ {c.q(d)} }};",
        reason=f"for over {y} annotated as [{x}]",
        confidence="high")


@recipe("axis_order_mismatch")
def r_axis_order(c):
    x, _, decl = _mkindex(c, 2)
    y, _, decl2 = _mkindex(c, 2)
    n, m = c.v(), c.v()
    mk = f"node {m}: Mass[{x}, {y}] = for a: {x}, b: {y} {{ {c.q('Mass')} }};"
    return dict(
        probe=f"{decl}\n{decl2}\n{mk}\nnode {n}: Mass[{x}, {y}] = for b: {y}, a: {x} {{ @{m}[a, b] }};",
        control=f"{decl}\n{decl2}\n{mk}\nnode {n}: Mass[{y}, {x}] = for b: {y}, a: {x} {{ @{m}[a, b] }};",
        reason="T[I, J] and T[J, I] are different types (axis order matters)",
        confidence="high")


@recipe("cross_needs_three")
def r_cross_three(c):
    n, m, m2 = c.v(), c.v(), c.v()
    k = c.rng.choice([2, 4, 5])
    mk = (f"node {m}: Length[Fin({k})] = for i: Fin({k}) {{ {c.q('Length')} }};\n"
          f"node {m2}: Force[Fin({k})] = for i: Fin({k}) {{ {c.q('Force')} }};")
    ok = (f"node {m}: Length[Fin(3)] = for i: Fin(3) {{ {c.q('Length')} }};\n"
          f"node {m2}: Force[Fin(3)] = for i: Fin(3) {{ {c.q('Force')} }};")
    return dict(
        probe=f"{mk}\nnode {n}: Length*Force[Fin({k})] = cross(@{m}, @{m2});",
        control=f"{ok}\nnode {n}: Length*Force[Fin(3)] = cross(@{m}, @{m2});",
        reason=f"cross requires a 3-entry axis, got Fin({k})",
        confidence="high")


@recipe("dot_axis_mismatch")
def r_dot_mismatch(c):
    x, _, decl = _mkindex(c, 3)
    y, _, decl2 = _mkindex(c, 3)
    n, m, m2 = c.v(), c.v(), c.v()
    mk = (f"node {m}: Length[{x}] = for a: {x} {{ {c.q('Length')} }};\n"
          f"node {m2}: Force[{y}] = for b: {y} {{ {c.q('Force')} }};")
    ok = (f"node {m}: Length[{x}] = for a: {x} {{ {c.q('Length')} }};\n"
          f"node {m2}: Force[{x}] = for a: {x} {{ {c.q('Force')} }};")
    return dict(
        probe=f"{decl}\n{decl2}\n{mk}\nnode {n}: Energy = dot(@{m}, @{m2});",
        control=f"{decl}\n{decl2}\n{ok}\nnode {n}: Energy = dot(@{m}, @{m2});",
        reason="dot requires both vectors on the same axis",
        confidence="high")


@recipe("trace_non_square")
def r_trace_nonsquare(c):
    x, _, decl = _mkindex(c, 2)
    y, _, decl2 = _mkindex(c, 2)
    n, m = c.v(), c.v()
    mk = f"node {m}: Mass[{x}, {y}] = for a: {x}, b: {y} {{ {c.q('Mass')} }};"
    ok = f"node {m}: Mass[{x}, {x}] = for a: {x}, b: {x} {{ {c.q('Mass')} }};"
    return dict(
        probe=f"{decl}\n{decl2}\n{mk}\nnode {n}: Mass = trace(@{m});",
        control=f"{decl}\n{decl2}\n{ok}\nnode {n}: Mass = trace(@{m});",
        reason="trace needs the same typed axis at both positions",
        confidence="high")


@recipe("matmul_inner_axis_mismatch")
def r_matmul_mismatch(c):
    x, _, dx = _mkindex(c, 2)
    y, _, dy = _mkindex(c, 2)
    z, _, dz = _mkindex(c, 2)
    w, _, dw = _mkindex(c, 2)
    n, a, b = c.v(), c.v(), c.v()
    mats = (f"node {a}: Mass[{x}, {y}] = for i: {x}, j: {y} {{ {c.q('Mass')} }};\n"
            f"node {b}: Time[{z}, {w}] = for i: {z}, j: {w} {{ {c.q('Time')} }};")
    ok = (f"node {a}: Mass[{x}, {y}] = for i: {x}, j: {y} {{ {c.q('Mass')} }};\n"
          f"node {b}: Time[{y}, {w}] = for i: {y}, j: {w} {{ {c.q('Time')} }};")
    return dict(
        probe=f"{dx}\n{dy}\n{dz}\n{dw}\n{mats}\nnode {n}: Mass*Time[{x}, {w}] = matmul(@{a}, @{b});",
        control=f"{dx}\n{dy}\n{dz}\n{dw}\n{ok}\nnode {n}: Mass*Time[{x}, {w}] = matmul(@{a}, @{b});",
        reason="matmul inner axes disagree",
        confidence="high")


@recipe("scan_multi_axis_source")
def r_scan_multi(c):
    x, _, dx = _mkindex(c, 2)
    y, _, dy = _mkindex(c, 2)
    n, m = c.v(), c.v()
    mk = f"node {m}: Mass[{x}, {y}] = for a: {x}, b: {y} {{ {c.q('Mass')} }};"
    ok = f"node {m}: Mass[{x}] = for a: {x} {{ {c.q('Mass')} }};"
    return dict(
        probe=(f"{dx}\n{dy}\n{mk}\n"
               f"node {n}: Mass[{x}, {y}] = scan(@{m}, 0.0 kg, |acc, item| acc + item);"),
        control=(f"{dx}\n{dy}\n{ok}\n"
                 f"node {n}: Mass[{x}] = scan(@{m}, 0.0 kg, |acc, item| acc + item);"),
        reason="scan sources have exactly one axis",
        confidence="high")


@recipe("scan_body_type_mismatch")
def r_scan_body(c):
    x, _, dx = _mkindex(c, 2)
    n, m = c.v(), c.v()
    mk = f"node {m}: Mass[{x}] = for a: {x} {{ {c.q('Mass')} }};"
    return dict(
        probe=(f"{dx}\n{mk}\n"
               f"node {n}: Mass[{x}] = scan(@{m}, 0.0 kg, |acc, item| acc / item);"),
        control=(f"{dx}\n{mk}\n"
                 f"node {n}: Mass[{x}] = scan(@{m}, 0.0 kg, |acc, item| acc + item);"),
        reason="scan body must return the accumulator type (Mass/Mass=Dimensionless)",
        confidence="high")


@recipe("range_dim_mismatch")
def r_range_dim(c):
    x = c.T()
    n = c.v()
    return dict(
        probe=(f"index {x} = range(0.0 s, 10.0 m, step: 1.0 s);\n"
               f"node {n}: Int = count(for t: {x} {{ coord(t) }});"),
        control=(f"index {x} = range(0.0 s, 10.0 s, step: 1.0 s);\n"
                 f"node {n}: Int = count(for t: {x} {{ coord(t) }});"),
        reason="range endpoints/step must share one dimension",
        confidence="high")


@recipe("range_zero_step")
def r_range_zero_step(c):
    x = c.T()
    n = c.v()
    return dict(
        probe=(f"index {x} = range(0.0 s, 10.0 s, step: 0.0 s);\n"
               f"node {n}: Int = count(for t: {x} {{ coord(t) }});"),
        control=(f"index {x} = range(0.0 s, 10.0 s, step: 2.5 s);\n"
                 f"node {n}: Int = count(for t: {x} {{ coord(t) }});"),
        reason="range with zero step cannot terminate",
        confidence="high")


@recipe("linspace_zero_points")
def r_linspace_zero(c):
    x = c.T()
    n = c.v()
    k = c.rng.choice([0, 1])
    return dict(
        probe=(f"index {x} = linspace(0.0 s, 10.0 s, points: {k});\n"
               f"node {n}: Int = count(for t: {x} {{ coord(t) }});"),
        control=(f"index {x} = linspace(0.0 s, 10.0 s, points: 5);\n"
                 f"node {n}: Int = count(for t: {x} {{ coord(t) }});"),
        reason=f"linspace with {k} point(s) is not a usable axis",
        confidence="med")


@recipe("unfold_on_named_index")
def r_unfold_named(c):
    x, _, decl = _mkindex(c, 3)
    n = c.v()
    ci = c.T()
    return dict(
        probe=(f"{decl}\n"
               f"node {n}: Dimensionless[{x}] = unfold({x}, 1.0, "
               f"|prev, pi, i| prev * 1.5);"),
        control=(f"index {ci} = linspace(0.0 s, 4.0 s, points: 5);\n"
                 f"node {n}: Dimensionless[{ci}] = unfold({ci}, 1.0, "
                 f"|prev, pi, i| prev * 1.5);"),
        reason="unfold's index must be a coordinate index",
        confidence="high")


# --- ADTs & generics --------------------------------------------------------

def _mkadt2(c):
    """Two-constructor ADT."""
    t = c.T()
    c1, c2 = c.T(), c.T()
    f1 = c.v()
    d1 = pick_dim(c.rng)
    decl = (f"type {t} {{\n    {c1}({f1}: {d1}),\n    {c2},\n}}")
    return t, c1, c2, f1, d1, decl


@recipe("match_ctor_nonexhaustive")
def r_match_nonex(c):
    t, c1, c2, f1, d1, decl = _mkadt2(c)
    n, w = c.v(), c.v()
    return dict(
        probe=(f"{decl}\nnode {w}: {t} = {c2};\n"
               f"node {n}: {d1} = match @{w} {{ {c1}({f1}: v) => v }};"),
        control=(f"{decl}\nnode {w}: {t} = {c2};\n"
                 f"node {n}: {d1} = match @{w} {{ {c1}({f1}: v) => v, "
                 f"{c2} => {c.q(d1)} }};"),
        reason=f"match missing constructor {c2}",
        confidence="high")


@recipe("match_arm_type_mismatch")
def r_match_arm_type(c):
    t, c1, c2, f1, d1, decl = _mkadt2(c)
    d2 = pick_dim(c.rng, exclude=(d1,))
    n, w = c.v(), c.v()
    return dict(
        probe=(f"{decl}\nnode {w}: {t} = {c2};\n"
               f"node {n}: {d1} = match @{w} {{ {c1}({f1}: v) => v, "
               f"{c2} => {c.q(d2)} }};"),
        control=(f"{decl}\nnode {w}: {t} = {c2};\n"
                 f"node {n}: {d1} = match @{w} {{ {c1}({f1}: v) => v, "
                 f"{c2} => {c.q(d1)} }};"),
        reason=f"match arms disagree: {d1} vs {d2}",
        confidence="high")


@recipe("field_access_multi_ctor")
def r_field_multi(c):
    t, c1, c2, f1, d1, decl = _mkadt2(c)
    n, w = c.v(), c.v()
    return dict(
        probe=(f"{decl}\nnode {w}: {t} = {c1}({f1}: {c.q(d1)});\n"
               f"node {n}: {d1} = @{w}.{f1};"),
        control=(f"{decl}\nnode {w}: {t} = {c1}({f1}: {c.q(d1)});\n"
                 f"node {n}: {d1} = match @{w} {{ {c1}({f1}: v) => v, "
                 f"{c2} => {c.q(d1)} }};"),
        reason="field access on a multi-constructor type",
        confidence="high")


@recipe("unknown_field_access")
def r_unknown_field(c):
    t = c.T()
    f1 = c.v()
    ghost = c.v()
    n, w = c.v(), c.v()
    d = pick_dim(c.rng)
    decl = f"type {t} {{ {t}({f1}: {d}) }}"
    return dict(
        probe=(f"{decl}\nnode {w}: {t} = {t}({f1}: {c.q(d)});\n"
               f"node {n}: {d} = @{w}.{ghost};"),
        control=(f"{decl}\nnode {w}: {t} = {t}({f1}: {c.q(d)});\n"
                 f"node {n}: {d} = @{w}.{f1};"),
        reason=f"unknown payload field .{ghost}",
        confidence="high")


@recipe("unknown_constructor")
def r_unknown_ctor(c):
    t, c1, c2, f1, d1, decl = _mkadt2(c)
    ghost = c.T()
    n = c.v()
    return dict(
        probe=f"{decl}\nnode {n}: {t} = {ghost};",
        control=f"{decl}\nnode {n}: {t} = {c2};",
        reason=f"unknown constructor {ghost}",
        confidence="high")


@recipe("ctor_missing_field")
def r_ctor_missing_field(c):
    t = c.T()
    f1, f2 = c.v(), c.v()
    n = c.v()
    d1, d2 = c.dim2()
    decl = f"type {t} {{ {t}({f1}: {d1}, {f2}: {d2}) }}"
    return dict(
        probe=f"{decl}\nnode {n}: {t} = {t}({f1}: {c.q(d1)});",
        control=f"{decl}\nnode {n}: {t} = {t}({f1}: {c.q(d1)}, {f2}: {c.q(d2)});",
        reason=f"constructor call missing field {f2}",
        confidence="high")


@recipe("ctor_positional_arg")
def r_ctor_positional(c):
    t = c.T()
    f1 = c.v()
    n = c.v()
    d = pick_dim(c.rng)
    decl = f"type {t} {{ {t}({f1}: {d}) }}"
    return dict(
        probe=f"{decl}\nnode {n}: {t} = {t}({c.q(d)});",
        control=f"{decl}\nnode {n}: {t} = {t}({f1}: {c.q(d)});",
        reason="constructor fields must be named",
        confidence="high")


@recipe("ctor_field_type_mismatch")
def r_ctor_field_type(c):
    t = c.T()
    f1 = c.v()
    n = c.v()
    d1, d2 = c.dim2()
    decl = f"type {t} {{ {t}({f1}: {d1}) }}"
    return dict(
        probe=f"{decl}\nnode {n}: {t} = {t}({f1}: {c.q(d2)});",
        control=f"{decl}\nnode {n}: {t} = {t}({f1}: {c.q(d1)});",
        reason=f"field {f1} expects {d1}, got {d2}",
        confidence="high")


@recipe("ctor_duplicate_field_arg")
def r_ctor_dup_field(c):
    t = c.T()
    f1, f2 = c.v(), c.v()
    n = c.v()
    d1, d2 = c.dim2()
    decl = f"type {t} {{ {t}({f1}: {d1}, {f2}: {d2}) }}"
    return dict(
        probe=(f"{decl}\nnode {n}: {t} = {t}({f1}: {c.q(d1)}, {f1}: {c.q(d1)}, "
               f"{f2}: {c.q(d2)});"),
        control=f"{decl}\nnode {n}: {t} = {t}({f1}: {c.q(d1)}, {f2}: {c.q(d2)});",
        reason=f"field {f1} supplied twice",
        confidence="high")


@recipe("duplicate_ctor_in_type")
def r_dup_ctor(c):
    t = c.T()
    a = c.T()
    b = c.T()
    return dict(
        probe=f"type {t} {{ {a}, {a}, {b} }}",
        control=f"type {t} {{ {a}, {b} }}",
        reason="duplicate constructor inside one type",
        confidence="high")


@recipe("duplicate_field_in_ctor")
def r_dup_field(c):
    t = c.T()
    f1 = c.v()
    d1, d2 = c.dim2()
    return dict(
        probe=f"type {t} {{ {t}({f1}: {d1}, {f1}: {d2}) }}",
        control=f"type {t} {{ {t}({f1}: {d1}, {f1}x: {d2}) }}",
        reason="duplicate payload field name in one constructor",
        confidence="high")


@recipe("generic_kind_mismatch")
def r_generic_kind(c):
    t = c.T()
    n = c.v()
    variant = c.rng.randrange(3)
    if variant == 0:
        decl = f"type {t}<D: Dim> {{ {t}(v: D) }}"
        probe = f"{decl}\nnode {n}: {t}<Fin(3)> = {t}<Fin(3)>(v: {c.f()} m);"
        control = f"{decl}\nnode {n}: {t}<Length> = {t}<Length>(v: {c.f()} m);"
        reason = "Index argument Fin(3) supplied for a Dim parameter"
    elif variant == 1:
        decl = f"type {t}<N: Nat> {{ {t}(v: Dimensionless) }}"
        probe = f"{decl}\nnode {n}: {t}<Length> = {t}<Length>(v: {c.f()});"
        control = f"{decl}\nnode {n}: {t}<3> = {t}<3>(v: {c.f()});"
        reason = "Dim argument Length supplied for a Nat parameter"
    else:
        x, _, ixdecl = _mkindex(c, 2)
        decl = f"type {t}<T: Type> {{ {t}(v: Bool) }}"
        probe = (f"{ixdecl}\n{decl}\nnode {n}: {t}<Mass[{x}]> = "
                 f"{t}<Mass[{x}]>(v: true);")
        control = f"{ixdecl}\n{decl}\nnode {n}: {t}<Mass> = {t}<Mass>(v: true);"
        reason = "indexed DeclType passed as a Type argument"
    return dict(probe=probe, control=control, reason=reason, confidence="high")


@recipe("generic_arity_mismatch")
def r_generic_arity(c):
    t = c.T()
    n = c.v()
    decl = f"type {t}<D: Dim> {{ {t}(v: D) }}"
    return dict(
        probe=(f"{decl}\nnode {n}: {t}<Length, Mass> = "
               f"{t}<Length, Mass>(v: {c.f()} m);"),
        control=f"{decl}\nnode {n}: {t}<Length> = {t}<Length>(v: {c.f()} m);",
        reason="too many generic arguments",
        confidence="high")


@recipe("generic_missing_args")
def r_generic_missing(c):
    t = c.T()
    n = c.v()
    decl = f"type {t}<D: Dim> {{ {t}(v: D) }}"
    return dict(
        probe=f"{decl}\nnode {n}: {t} = {t}(v: {c.f()} m);",
        control=f"{decl}\nnode {n}: {t}<Length> = {t}<Length>(v: {c.f()} m);",
        reason="generic type used without required arguments",
        confidence="high")


@recipe("empty_generic_brackets")
def r_empty_generics(c):
    t = c.T()
    n = c.v()
    decl = f"type {t}<D: Dim = Length> {{ {t}(v: D) }}"
    return dict(
        probe=f"{decl}\nnode {n}: {t}<> = {t}<>(v: {c.f()} m);",
        control=f"{decl}\nnode {n}: {t} = {t}(v: {c.f()} m);",
        reason="empty angle brackets are invalid",
        confidence="high")


@recipe("fn_generic_args")
def r_fn_generic(c):
    n = c.v()
    return dict(
        probe=f"node {n}: Dimensionless = sqrt<3>({c.f()});",
        control=f"node {n}: Dimensionless = sqrt({c.f()});",
        reason="ordinary functions take no generic arguments (sqrt<3>)",
        confidence="high")


@recipe("constraint_on_generic_arg")
def r_constraint_generic(c):
    t = c.T()
    n = c.v()
    decl = f"type {t}<D: Dim> {{ {t}(v: D) }}"
    return dict(
        probe=(f"{decl}\nnode {n}: {t}<Length(min: 0.0 m)> = "
               f"{t}<Length>(v: {c.f()} m);"),
        control=f"{decl}\nnode {n}: {t}<Length> = {t}<Length>(v: {c.f()} m);",
        reason="domain constraint on a generic type argument",
        confidence="high")


@recipe("nontrailing_default")
def r_nontrailing_default(c):
    t = c.T()
    return dict(
        probe=f"type {t}<A: Dim = Length, B: Dim> {{ {t}(x: A, y: B) }}",
        control=f"type {t}<A: Dim, B: Dim = Length> {{ {t}(x: A, y: B) }}",
        reason="defaulted generic parameters must form a trailing suffix",
        confidence="high")


@recipe("phantom_frame_mismatch")
def r_phantom_mismatch(c):
    v3, eci, body = c.T(), c.T(), c.T()
    n, m = c.v(), c.v()
    decls = (f"type {eci} {{ {eci} }}\n"
             f"type {body} {{ {body} }}\n"
             f"type {v3}<D: Dim, F: Type> {{ {v3}(x: D, y: D) }}")
    mk = (f"node {m}: {v3}<Length, {eci}> = "
          f"{v3}<Length, {eci}>(x: {c.q('Length')}, y: {c.q('Length')});")
    return dict(
        probe=f"{decls}\n{mk}\nnode {n}: {v3}<Length, {body}> = @{m};",
        control=f"{decls}\n{mk}\nnode {n}: {v3}<Length, {eci}> = @{m};",
        reason="phantom frame parameters make the types distinct",
        confidence="high")


# --- domain constraints -----------------------------------------------------

@recipe("constraint_on_bool")
def r_constraint_bool(c):
    n = c.v()
    return dict(
        probe=f"param {n}: Bool(min: 0) = true;",
        control=f"param {n}: Bool = true;",
        reason="domain constraints are invalid on Bool",
        confidence="high")


@recipe("constraint_unknown_key")
def r_constraint_key(c):
    n = c.v()
    kw = c.rng.choice(["step", "mean", "mode", "span"])
    return dict(
        probe=f"param {n}: Mass({kw}: {c.q('Mass')}) = {c.q('Mass')};",
        control=f"param {n}: Mass(min: 0.0 kg) = {c.q('Mass')};",
        reason=f"unknown constraint key `{kw}`",
        confidence="high")


@recipe("constraint_min_gt_max")
def r_constraint_mingtmax(c):
    n = c.v()
    d = pick_dim(c.rng, exclude=("Dimensionless",))
    un = c.u(d)
    lo, hi = sorted([c.rng.uniform(1, 50), c.rng.uniform(51, 500)])
    return dict(
        probe=f"param {n}: {d}(min: {hi:.1f} {un}, max: {lo:.1f} {un}) = {lo:.1f} {un};",
        control=f"param {n}: {d}(min: {lo:.1f} {un}, max: {hi:.1f} {un}) = {hi:.1f} {un};",
        reason="min exceeds max in a domain constraint",
        confidence="high")


@recipe("constraint_bound_dim_mismatch")
def r_constraint_dim(c):
    n = c.v()
    d1, d2 = c.dim2()
    return dict(
        probe=f"param {n}: {d1}(min: {c.q(d2, 0.0, 1.0)}) = {c.q(d1)};",
        control=f"param {n}: {d1}(min: {c.q(d1, 0.0, 0.01)}) = {c.q(d1, 10.0, 999.0)};",
        reason=f"constraint bound of dimension {d2} on a {d1} type",
        confidence="high")


@recipe("constraint_int_bound_float")
def r_constraint_int(c):
    n = c.v()
    return dict(
        probe=f"param {n}: Int(min: {c.f(0.0, 3.0)}) = {ilit(c.rng, 5, 50)};",
        control=f"param {n}: Int(min: 1) = {ilit(c.rng, 5, 50)};",
        reason="Int bounds must be exactly Int",
        confidence="high")


@recipe("constraint_bound_reads_param")
def r_constraint_nonconst(c):
    n, p = c.v(), c.v()
    return dict(
        probe=(f"param {p}: Mass = {c.q('Mass')};\n"
               f"param {n}: Mass(min: @{p}) = {c.q('Mass')};"),
        control=(f"const node {p}: Mass = {c.q('Mass', 0.001, 0.01)};\n"
                 f"param {n}: Mass(min: @{p}) = {c.q('Mass', 10.0, 999.0)};"),
        reason="constraint bounds cannot read params or runtime nodes",
        confidence="high")


@recipe("constraint_on_adt")
def r_constraint_adt(c):
    t = c.T()
    n = c.v()
    decl = f"pub type {t} {{ {t}(v: Mass) }}"
    return dict(
        probe=(f"{decl}\nparam {n}: {t}(min: {c.q('Mass')}) = "
               f"{t}(v: {c.q('Mass')});"),
        control=f"{decl}\nparam {n}: {t} = {t}(v: {c.q('Mass')});",
        reason="domain constraints are invalid on algebraic types",
        confidence="high")


@recipe("const_domain_violation")
def r_const_domain_violation(c):
    n = c.v()
    d = pick_dim(c.rng, exclude=("Dimensionless",))
    un = c.u(d)
    return dict(
        probe=(f"const node {n}: {d}(min: 10.0 {un}) = 1.0 {un};"),
        control=(f"const node {n}: {d}(min: 10.0 {un}) = 100.0 {un};"),
        reason="const node values violating their domain are caught at compile time",
        confidence="high")


@recipe("datetime_bound_scale_mismatch")
def r_dt_bound_scale(c):
    n = c.v()
    return dict(
        probe=(f'param {n}: Datetime<TT>(min: datetime("2024-01-01T00:00:00Z")) '
               f'= epoch<TT>("2024-06-01T00:00:00");'),
        control=(f'param {n}: Datetime<TT>(min: epoch<TT>("2024-01-01T00:00:00")) '
                 f'= epoch<TT>("2024-06-01T00:00:00");'),
        reason="Datetime<TT> bound must be exactly Datetime<TT>",
        confidence="high")


# --- datetimes --------------------------------------------------------------

@recipe("datetime_cross_scale_op")
def r_dt_cross_scale(c):
    n = c.v()
    return dict(
        probe=(f'node {n}: Time = epoch<TT>("2024-06-01T00:00:00") - '
               f'datetime("2024-01-01T00:00:00Z");'),
        control=(f'node {n}: Time = epoch<TT>("2024-06-01T00:00:00") - '
                 f'epoch<TT>("2024-01-01T00:00:00");'),
        reason="cross-scale datetime arithmetic without explicit conversion",
        confidence="high")


@recipe("datetime_plus_datetime")
def r_dt_plus_dt(c):
    n = c.v()
    return dict(
        probe=(f'node {n}: Time = datetime("2024-06-01T00:00:00Z") + '
               f'datetime("2024-01-01T00:00:00Z");'),
        control=(f'node {n}: Time = datetime("2024-06-01T00:00:00Z") - '
                 f'datetime("2024-01-01T00:00:00Z");'),
        reason="Datetime + Datetime is not defined",
        confidence="high")


@recipe("time_minus_datetime")
def r_time_minus_dt(c):
    n = c.v()
    return dict(
        probe=f'node {n}: Datetime = {c.q("Time")} - datetime("2024-01-01T00:00:00Z");',
        control=f'node {n}: Datetime = datetime("2024-01-01T00:00:00Z") - {c.q("Time")};',
        reason="Time - Datetime is not defined",
        confidence="high")


@recipe("datetime_invalid_date")
def r_dt_invalid_date(c):
    n = c.v()
    bad = c.rng.choice(["2024-02-30T00:00:00Z", "2023-02-29T10:00:00Z",
                        "2024-13-01T00:00:00Z", "2024-04-31T12:30:00Z",
                        "2024-01-01T24:01:00Z"])
    return dict(
        probe=f'node {n}: Datetime = datetime("{bad}");',
        control=f'node {n}: Datetime = datetime("2024-02-29T00:00:00Z");',
        reason=f"nonexistent civil date/time `{bad}`",
        confidence="high")


@recipe("datetime_missing_offset")
def r_dt_missing_offset(c):
    n = c.v()
    return dict(
        probe=f'node {n}: Datetime = datetime("2024-06-01T00:00:00");',
        control=f'node {n}: Datetime = datetime("2024-06-01T00:00:00Z");',
        reason="one-argument datetime requires an explicit offset",
        confidence="high")


@recipe("epoch_with_offset")
def r_epoch_offset(c):
    n = c.v()
    return dict(
        probe=f'node {n}: Datetime<TT> = epoch<TT>("2024-06-01T00:00:00Z");',
        control=f'node {n}: Datetime<TT> = epoch<TT>("2024-06-01T00:00:00");',
        reason="epoch accepts only offset/zone-free coordinates",
        confidence="high")


@recipe("datetime_date_only")
def r_dt_date_only(c):
    n = c.v()
    return dict(
        probe=f'node {n}: Datetime = datetime("2024-06-01Z");',
        control=f'node {n}: Datetime = datetime("2024-06-01T00:00:00Z");',
        reason="date-only strings are rejected",
        confidence="high")


@recipe("datetime_bad_timezone")
def r_dt_bad_tz(c):
    n = c.v()
    tz = c.rng.choice(["Mars/Olympus", "America/NotACity", "Foo/Bar"])
    return dict(
        probe=f'node {n}: Datetime = datetime("2024-06-01T10:00:00", "{tz}");',
        control=f'node {n}: Datetime = datetime("2024-06-01T10:00:00", "America/New_York");',
        reason=f"unknown IANA timezone `{tz}`",
        confidence="high")


@recipe("datetime_dst_gap")
def r_dt_dst_gap(c):
    n = c.v()
    return dict(
        probe=f'node {n}: Datetime = datetime("2025-03-09T02:30:00", "America/New_York");',
        control=f'node {n}: Datetime = datetime("2025-03-09T03:30:00", "America/New_York");',
        reason="nonexistent DST-gap civil time",
        confidence="high")


@recipe("datetime_scale_annotation_mismatch")
def r_dt_scale_annot(c):
    n = c.v()
    return dict(
        probe=f'node {n}: Datetime<TT> = datetime("2024-06-01T00:00:00Z");',
        control=f'node {n}: Datetime<TT> = to_tt(datetime("2024-06-01T00:00:00Z"));',
        reason="UTC datetime under a Datetime<TT> annotation",
        confidence="high")


@recipe("datetime_extract_wrong_type")
def r_dt_extract(c):
    n = c.v()
    fn = c.rng.choice(["year", "month", "day", "hour", "weekday"])
    return dict(
        probe=f"node {n}: Int = {fn}({c.f()});",
        control=f'node {n}: Int = {fn}(datetime("2024-06-01T00:00:00Z"));',
        reason=f"{fn} requires a Datetime argument",
        confidence="high")


@recipe("datetime_ordering_cross_scale")
def r_dt_cmp_cross(c):
    n = c.v()
    return dict(
        probe=(f'node {n}: Bool = datetime("2024-01-01T00:00:00Z") < '
               f'epoch<GPST>("2024-06-01T00:00:00");'),
        control=(f'node {n}: Bool = datetime("2024-01-01T00:00:00Z") < '
                 f'datetime("2024-06-01T00:00:00Z");'),
        reason="ordering comparison across datetime scales",
        confidence="high")


# --- complex ----------------------------------------------------------------

@recipe("complex_bare")
def r_complex_bare(c):
    n = c.v()
    return dict(
        probe=f"node {n}: Complex = complex({c.f()}, {c.f()});",
        control=f"node {n}: Complex<Dimensionless> = complex({c.f()}, {c.f()});",
        reason="bare Complex without a dimension argument",
        confidence="high")


@recipe("complex_unit_arg")
def r_complex_unit_arg(c):
    n = c.v()
    return dict(
        probe=f"node {n}: Complex<m> = complex({c.q('Length')}, {c.q('Length')});",
        control=f"node {n}: Complex<Length> = complex({c.q('Length')}, {c.q('Length')});",
        reason="Complex takes a dimension, not a unit",
        confidence="high")


@recipe("complex_component_mismatch")
def r_complex_comp(c):
    n = c.v()
    d1, d2 = c.dim2()
    return dict(
        probe=f"node {n}: Complex<{d1}> = complex({c.q(d1)}, {c.q(d2)});",
        control=f"node {n}: Complex<{d1}> = complex({c.q(d1)}, {c.q(d1)});",
        reason="complex components must share one dimension",
        confidence="high")


@recipe("complex_mixed_add")
def r_complex_mixed_add(c):
    n = c.v()
    return dict(
        probe=(f"node {n}: Complex<Length> = complex({c.q('Length')}, "
               f"{c.q('Length')}) + {c.q('Length')};"),
        control=(f"node {n}: Complex<Length> = complex({c.q('Length')}, "
                 f"{c.q('Length')}) + to_complex({c.q('Length')});"),
        reason="real operands are never promoted implicitly for +",
        confidence="high")


@recipe("complex_ordering")
def r_complex_ordering(c):
    n = c.v()
    op = c.rng.choice(["<", ">", "<=", ">="])
    return dict(
        probe=(f"node {n}: Bool = complex({c.f()}, {c.f()}) {op} "
               f"complex({c.f()}, {c.f()});"),
        control=(f"node {n}: Bool = complex({c.f()}, {c.f()}) == "
                 f"complex({c.f()}, {c.f()});"),
        reason="complex values are unordered",
        confidence="high")


@recipe("complex_exp_dimensioned")
def r_complex_exp_dim(c):
    n = c.v()
    return dict(
        probe=(f"node {n}: Complex<Length> = exp(complex({c.q('Length')}, "
               f"{c.q('Length')}));"),
        control=(f"node {n}: Complex<Dimensionless> = exp(complex({c.f()}, "
                 f"{c.f()}));"),
        reason="complex exp requires Complex<Dimensionless>",
        confidence="high")


@recipe("complex_modulo")
def r_complex_mod(c):
    n = c.v()
    op = c.rng.choice(["%", "^"])
    rhs = "2" if op == "^" else f"complex({c.f()}, {c.f()})"
    return dict(
        probe=(f"node {n}: Complex<Dimensionless> = complex({c.f()}, {c.f()}) "
               f"{op} {rhs};"),
        control=(f"node {n}: Complex<Dimensionless> = complex({c.f()}, {c.f()}) "
                 f"* complex({c.f()}, {c.f()});"),
        reason=f"`{op}` is not defined for complex operands",
        confidence="high")


# --- dag / include / import -------------------------------------------------

def _mkdag(c, pub=False):
    dg, pa, no = c.v(), c.v(), c.v()
    d = pick_dim(c.rng)
    vis = "pub " if pub else ""
    decl = (f"dag {dg} {{\n    param {pa}: {d};\n"
            f"    {vis}node {no}: {d} = @{pa} * 2.0;\n}}")
    return dg, pa, no, d, decl


@recipe("include_unknown_dag")
def r_include_unknown(c):
    dg, pa, no, d, decl = _mkdag(c)
    ghost = c.v()
    al = c.v()
    return dict(
        probe=f"{decl}\ninclude {ghost}({pa}: {c.q(d)}).{{ {no} as {al} }};",
        control=f"{decl}\ninclude {dg}({pa}: {c.q(d)}).{{ {no} as {al} }};",
        reason=f"include of unknown dag `{ghost}`",
        confidence="high")


@recipe("include_missing_required_param")
def r_include_missing_param(c):
    dg, pa, no, al = c.v(), c.v(), c.v(), c.v()
    d = pick_dim(c.rng)
    decl = (f"dag {dg} {{\n    param {pa}: {d};\n"
            f"    node {no}: {d} = @{pa} * 2.0;\n}}")
    return dict(
        probe=f"{decl}\ninclude {dg}().{{ {no} as {al} }};",
        control=f"{decl}\ninclude {dg}({pa}: {c.q(d)}).{{ {no} as {al} }};",
        reason=f"required dag param {pa} left unbound",
        confidence="high")


@recipe("include_unknown_binding")
def r_include_unknown_binding(c):
    dg, pa, no, d, decl = _mkdag(c)
    ghost = c.v()
    al = c.v()
    return dict(
        probe=(f"{decl}\ninclude {dg}({pa}: {c.q(d)}, {ghost}: {c.f()})"
               f".{{ {no} as {al} }};"),
        control=f"{decl}\ninclude {dg}({pa}: {c.q(d)}).{{ {no} as {al} }};",
        reason=f"binding for nonexistent port `{ghost}`",
        confidence="high")


@recipe("include_duplicate_binding")
def r_include_dup_binding(c):
    dg, pa, no, d, decl = _mkdag(c)
    al = c.v()
    return dict(
        probe=(f"{decl}\ninclude {dg}({pa}: {c.q(d)}, {pa}: {c.q(d)})"
               f".{{ {no} as {al} }};"),
        control=f"{decl}\ninclude {dg}({pa}: {c.q(d)}).{{ {no} as {al} }};",
        reason=f"port {pa} bound twice in one include",
        confidence="high")


@recipe("include_binding_dim_mismatch")
def r_include_binding_dim(c):
    dg, pa, no, al = c.v(), c.v(), c.v(), c.v()
    d1, d2 = c.dim2()
    decl = (f"dag {dg} {{\n    param {pa}: {d1};\n"
            f"    node {no}: {d1} = @{pa} * 2.0;\n}}")
    return dict(
        probe=f"{decl}\ninclude {dg}({pa}: {c.q(d2)}).{{ {no} as {al} }};",
        control=f"{decl}\ninclude {dg}({pa}: {c.q(d1)}).{{ {no} as {al} }};",
        reason=f"binding of {d2} value to a {d1} port",
        confidence="high")


@recipe("project_unknown_output")
def r_project_unknown(c):
    dg, pa, no, d, decl = _mkdag(c)
    ghost, al = c.v(), c.v()
    return dict(
        probe=f"{decl}\ninclude {dg}({pa}: {c.q(d)}).{{ {ghost} as {al} }};",
        control=f"{decl}\ninclude {dg}({pa}: {c.q(d)}).{{ {no} as {al} }};",
        reason=f"projecting unknown output `{ghost}`",
        confidence="high")


@recipe("dag_call_in_const")
def r_dag_call_const(c):
    dg, pa, no, d, decl = _mkdag(c, pub=True)
    n = c.v()
    return dict(
        probe=f"{decl}\nconst node {n}: {d} = @{dg}({pa}: {c.q(d)}).{no};",
        control=f"{decl}\nnode {n}: {d} = @{dg}({pa}: {c.q(d)}).{no};",
        reason="DAG calls are runtime includes; prohibited in const nodes",
        confidence="high")


@recipe("dag_call_no_projection")
def r_dag_call_no_proj(c):
    dg, pa, no, d, decl = _mkdag(c, pub=True)
    n = c.v()
    return dict(
        probe=f"{decl}\nnode {n}: {d} = @{dg}({pa}: {c.q(d)});",
        control=f"{decl}\nnode {n}: {d} = @{dg}({pa}: {c.q(d)}).{no};",
        reason="the projected output after a DAG call is mandatory",
        confidence="high")


@recipe("plain_ref_to_dag")
def r_plain_ref_dag(c):
    dg, pa, no, d, decl = _mkdag(c, pub=True)
    n = c.v()
    return dict(
        probe=f"{decl}\nnode {n}: {d} = @{dg};",
        control=f"{decl}\nnode {n}: {d} = @{dg}({pa}: {c.q(d)}).{no};",
        reason="a dag is not a value; bare @dag reference is invalid",
        confidence="high")


@recipe("import_nonexistent_module")
def r_import_missing(c):
    ghost = fresh(c.rng)
    n = c.v()
    return dict(
        probe=f"import {ghost};\nnode {n}: Dimensionless = {c.f()};",
        control=f"node {n}: Dimensionless = {c.f()};",
        reason=f"import of nonexistent module `{ghost}`",
        confidence="high")


@recipe("import_plugin_missing")
def r_import_plugin(c):
    ns = fresh(c.rng)
    n = c.v()
    return dict(
        probe=(f'import plugin "{fresh(c.rng)}" as {ns} {{\n'
               f"    fn f(x: Length) -> Length;\n}}\n"
               f"node {n}: Length = {ns}.f({c.q('Length')});"),
        control=f"node {n}: Length = {c.q('Length')};",
        reason="import of a nonexistent plugin",
        confidence="high")


@recipe("include_cycle_self")
def r_include_cycle(c):
    dg, pa = c.v(), c.v()
    no, al = c.v(), c.v()
    d = pick_dim(c.rng)
    return dict(
        probe=(f"dag {dg} {{\n    param {pa}: {d};\n"
               f"    node {no}: {d} = @{dg}({pa}: @{pa}).{no};\n}}\n"
               f"include {dg}({pa}: {c.q(d)}).{{ {no} as {al} }};"),
        control=(f"dag {dg} {{\n    param {pa}: {d};\n"
                 f"    node {no}: {d} = @{pa} * 2.0;\n}}\n"
                 f"include {dg}({pa}: {c.q(d)}).{{ {no} as {al} }};"),
        reason="dag recursively instantiates itself",
        confidence="high")


# --- plots ------------------------------------------------------------------

@recipe("plot_unknown_ref")
def r_plot_unknown(c):
    x, ls, decl = _mkindex(c, 2)
    pl, m = c.v(), c.v()
    ghost = c.v()
    mk = f"node {m}: Dimensionless[{x}] = for a: {x} {{ {c.f()} }};"
    return dict(
        probe=(f"{decl}\n{mk}\nplot {pl} = {{\n    mark: line,\n"
               f"    encode: {{ x: for a: {x} {{ @{ghost}[a] }}, "
               f"y: for a: {x} {{ @{m}[a] }} }},\n}};"),
        control=(f"{decl}\n{mk}\nplot {pl} = {{\n    mark: line,\n"
                 f"    encode: {{ x: for a: {x} {{ @{m}[a] }}, "
                 f"y: for a: {x} {{ @{m}[a] }} }},\n}};"),
        reason=f"plot encodes unknown reference @{ghost}",
        confidence="high")


@recipe("figure_unknown_plot")
def r_figure_unknown(c):
    x, ls, decl = _mkindex(c, 2)
    pl, fg, m = c.v(), c.v(), c.v()
    ghost = c.v()
    mk = f"node {m}: Dimensionless[{x}] = for a: {x} {{ {c.f()} }};"
    plot = (f"plot {pl} = {{\n    mark: line,\n"
            f"    encode: {{ x: for a: {x} {{ @{m}[a] }}, "
            f"y: for a: {x} {{ @{m}[a] }} }},\n}};")
    return dict(
        probe=f"{decl}\n{mk}\n{plot}\nfigure {fg} = {{ plots: [{ghost}] }};",
        control=f"{decl}\n{mk}\n{plot}\nfigure {fg} = {{ plots: [{pl}] }};",
        reason=f"figure references unknown plot `{ghost}`",
        confidence="high")


# --- parse-level probes -----------------------------------------------------

@recipe("cmp_chain")
def r_cmp_chain(c):
    n = c.v()
    return dict(
        probe=f"node {n}: Bool = {c.f()} < {c.f()} < {c.f()};",
        control=f"node {n}: Bool = {c.f(0.0, 1.0)} < {c.f(2.0, 3.0)} && {c.f(2.0, 3.0)} < {c.f(4.0, 9.0)};",
        reason="comparisons are non-chaining",
        confidence="high")


@recipe("arrow_chain_bare")
def r_arrow_chain(c):
    n, p = c.v(), c.v()
    return dict(
        probe=(f"param {p}: Length = {c.q('Length')};\n"
               f"node {n}: Length = @{p} -> km -> m;"),
        control=(f"param {p}: Length = {c.q('Length')};\n"
                 f"node {n}: Length = @{p} -> km;"),
        reason="chained -> is a parse error",
        confidence="high")


@recipe("empty_index_access")
def r_empty_access(c):
    x, ls, decl = _mkindex(c, 2)
    n, m = c.v(), c.v()
    mk = f"node {m}: Mass[{x}] = for a: {x} {{ {c.q('Mass')} }};"
    return dict(
        probe=f"{decl}\n{mk}\nnode {n}: Mass = @{m}[];",
        control=f"{decl}\n{mk}\nnode {n}: Mass = @{m}[{x}.{ls[0]}];",
        reason="expr[] is invalid",
        confidence="high")


@recipe("missing_semicolon")
def r_missing_semi(c):
    n, m = c.v(), c.v()
    return dict(
        probe=(f"node {n}: Dimensionless = {c.f()}\n"
               f"node {m}: Dimensionless = {c.f()};"),
        control=(f"node {n}: Dimensionless = {c.f()};\n"
                 f"node {m}: Dimensionless = {c.f()};"),
        reason="missing semicolon between declarations",
        confidence="high")


@recipe("unbalanced_parens")
def r_unbalanced(c):
    n = c.v()
    variant = c.rng.choice([
        f"node {n}: Dimensionless = ({c.f()} + {c.f()};",
        f"node {n}: Dimensionless = {c.f()} + {c.f()});",
        f"node {n}: Dimensionless[Fin(2) = for i: Fin(2) {{ {c.f()} }};",
    ])
    return dict(
        probe=variant,
        control=f"node {n}: Dimensionless = ({c.f()} + {c.f()});",
        reason="unbalanced brackets",
        confidence="high")


@recipe("block_comment")
def r_block_comment(c):
    n = c.v()
    return dict(
        probe=f"/* not a comment form */\nnode {n}: Dimensionless = {c.f()};",
        control=f"// fine\nnode {n}: Dimensionless = {c.f()};",
        reason="graphcal has no block comments",
        confidence="high")


@recipe("keyword_as_name")
def r_keyword_name(c):
    kw = c.rng.choice(["match", "node", "param", "index", "type", "if",
                       "else", "for", "assert", "include", "import", "dag"])
    return dict(
        probe=f"node {kw}: Dimensionless = {flit(c.rng)};",
        control=f"node {kw}_x: Dimensionless = {flit(c.rng)};",
        reason=f"keyword `{kw}` used as a declaration name",
        confidence="med")


@recipe("unicode_identifier")
def r_unicode_ident(c):
    n = c.rng.choice(["héllo", "π_value", "データ", "x²", "ميم", "ぱらむ"])
    return dict(
        probe=f"node {n}: Dimensionless = {c.f()};",
        control=f"node ascii_{_counter[0]}: Dimensionless = {c.f()};",
        reason=f"non-ASCII identifier `{n}`",
        confidence="med")


@recipe("bad_number_literal")
def r_bad_number(c):
    n = c.v()
    # NOTE: DIGIT_SEQ = DIGIT, { DIGIT | "_" } makes 1__000 / 1000_ / 1_.0
    # grammar-legal, so those spellings are NOT probes.
    lit = c.rng.choice(["1..0", "1.", ".5", "0x1F", "1e", "1.0e+"])
    return dict(
        probe=f"node {n}: Dimensionless = {lit};",
        control=f"node {n}: Dimensionless = 1.0;",
        reason=f"malformed numeric literal `{lit}`",
        confidence="med")


@recipe("string_in_expr")
def r_string_expr(c):
    n = c.v()
    return dict(
        probe=f'node {n}: Dimensionless = "text";',
        control=f"node {n}: Dimensionless = {c.f()};",
        reason="string literal used as a value (no String type)",
        confidence="high")


@recipe("type_name_in_expr")
def r_type_in_expr(c):
    n = c.v()
    d = pick_dim(c.rng, exclude=("Dimensionless",))
    return dict(
        probe=f"node {n}: {d} = {d};",
        control=f"node {n}: {d} = {c.q(d)};",
        reason="a dimension name is not a value",
        confidence="high")


@recipe("unit_name_in_expr")
def r_unit_in_expr(c):
    n = c.v()
    return dict(
        probe=f"node {n}: Length = m;",
        control=f"node {n}: Length = 1.0 m;",
        reason="a unit name is not a value",
        confidence="high")


@recipe("at_in_attribute")
def r_at_in_attr(c):
    n, a, p = c.v(), c.v(), c.v()
    return dict(
        probe=(f"param {p}: Dimensionless = {c.f()};\n"
               f"assert {a} = @{p} > 0.0;\n"
               f"#[assumes(@{a})]\n"
               f"node {n}: Dimensionless = {c.f()};"),
        control=(f"param {p}: Dimensionless = {c.f()};\n"
                 f"assert {a} = @{p} > 0.0;\n"
                 f"#[assumes({a})]\n"
                 f"node {n}: Dimensionless = {c.f()};"),
        reason="@ sigil inside an attribute argument",
        confidence="high")


@recipe("expected_fail_bare_int_key")
def r_ef_bare_int(c):
    a, m = c.v(), c.v()
    k = 3
    mk = f"node {m}: Dimensionless[Fin({k})] = for i: Fin({k}) {{ {c.f()} }};"
    return dict(
        probe=(f"{mk}\n#[expected_fail(1)]\n"
               f"assert {a} = for i: Fin({k}) {{ @{m}[i] > 0.0 }};"),
        control=(f"{mk}\n#[expected_fail(#1)]\n"
                 f"assert {a} = for i: Fin({k}) {{ @{m}[i] > 0.0 }};"),
        reason="positional expected_fail keys need the # prefix",
        confidence="high")


@recipe("multi_decl_missing_table")
def r_multi_decl(c):
    a, b = c.v(), c.v()
    x, ls, decl = _mkindex(c, 2)
    return dict(
        probe=(f"{decl}\nparam {a}: Mass[{x}], node {b}: Time[{x}] = "
               f"{{ {x}.{ls[0]}: {c.q('Mass')}, {x}.{ls[1]}: {c.q('Mass')} }};"),
        control=None,
        reason="multi-declaration requires a table literal covering all slots",
        confidence="med")


# --- entropy / stress -------------------------------------------------------

@recipe("deep_nesting_invalid_core")
def r_deep_nesting(c):
    n = c.v()
    ghost = c.v() + "_ghost"
    depth = c.rng.randint(20, 44)
    expr = "(" * depth + f"@{ghost}" + ")" * depth
    return dict(
        probe=f"node {n}: Dimensionless = {expr};",
        control=("node %s: Dimensionless = %s;"
                 % (n, "(" * depth + "1.0" + ")" * depth)),
        reason=f"undefined reference at paren depth {depth} must still be caught",
        confidence="high")


@recipe("long_chain_tail_error")
def r_long_chain(c):
    n = c.v()
    k = c.rng.randint(300, 1200)
    terms = " + ".join("1.0" for _ in range(k))
    return dict(
        probe=f"node {n}: Dimensionless = {terms} + {ilit(c.rng)};",
        control=f"node {n}: Dimensionless = {terms};",
        reason=f"Int literal at the tail of a {k}-term float chain",
        confidence="high")


@recipe("many_decls_one_dup")
def r_many_decls(c):
    k = c.rng.randint(300, 900)
    names = [f"bulk_{_counter[0]}_{i}" for i in range(k)]
    decls = [f"node {nm}: Dimensionless = {c.f()};" for nm in names]
    dup_i = c.rng.randrange(k)
    probe = decls + [f"node {names[dup_i]}: Dimensionless = {c.f()};"]
    return dict(
        probe="\n".join(probe),
        control="\n".join(decls),
        reason=f"one duplicate among {k + 1} declarations",
        confidence="high")


@recipe("very_long_identifier")
def r_long_ident(c):
    n = c.v()
    ghost = "g" + "x" * c.rng.randint(2000, 9000)
    return dict(
        probe=f"node {n}: Dimensionless = @{ghost};",
        control=f"node {n}: Dimensionless = {c.f()};",
        reason="reference to an undefined, pathologically long identifier",
        confidence="high")


def chaos_file(rng):
    """Random token soup that cannot be a valid program."""
    vocab = ["node", "param", "const", "index", "type", "dim", "unit", "dag",
             "match", "if", "else", "for", "include", "import", "assert",
             "{", "}", "(", ")", "[", "]", ";", ":", "=", "=>", "->", "@",
             "+", "-", "*", "/", "^", "%", "<", ">", "==", "&&", "||", "#[",
             "]", ",", ".", "true", "false", "Fin", "Key", "Complex",
             "⟦", "⟧", "∀", "∃", "λ", "×", "≠", "§", "€", "🚀", "\\", "$",
             "?", "~", "`", "'", '"']
    for _ in range(rng.randint(3, 10)):
        vocab.append(fresh(rng))
        vocab.append(flit(rng))
    lines = []
    for _ in range(rng.randint(2, 25)):
        k = rng.randint(3, 30)
        lines.append(" ".join(rng.choice(vocab) for _ in range(k)))
    # Guarantee at least one unmistakably-broken token at the start.
    return "⟁ " + "\n".join(lines)


# ------------------------------------------------------------------- driver

def build_file(rng, body, header):
    fill = filler_decls(rng, rng.randint(0, 4))
    parts = fill + [body]
    rng.shuffle(parts)
    return header + "\n\n" + "\n\n".join(parts) + "\n"


def main():
    rng = random.Random(SEED)
    if os.path.exists(OUT):
        shutil.rmtree(OUT)
    for sub in ("probes", "controls", "chaos"):
        os.makedirs(os.path.join(OUT, sub))
    manifest = []

    per_family = int(os.environ.get("GEN_PER_FAMILY", "6"))
    for fam, fn in sorted(RECIPES.items()):
        for i in range(per_family):
            c = Ctx(rng)
            try:
                r = fn(c)
            except Exception as e:  # recipe bug; skip loudly
                print(f"recipe {fam} crashed: {e}", file=sys.stderr)
                break
            stem = f"{fam}_{i:02d}"
            hdr = (f"// PROBE family={fam} case={i}\n"
                   f"// intended-error: {r['reason']}")
            pf = os.path.join("probes", stem + ".gcl")
            with open(os.path.join(OUT, pf), "w") as f:
                f.write(build_file(rng, r["probe"], hdr))
            manifest.append(dict(file=pf, family=fam, expected="fail",
                                 reason=r["reason"],
                                 confidence=r["confidence"]))
            if r.get("control"):
                cf = os.path.join("controls", stem + ".gcl")
                with open(os.path.join(OUT, cf), "w") as f:
                    f.write(build_file(
                        rng, r["control"],
                        f"// CONTROL for {fam} case={i} (must pass)"))
                manifest.append(dict(file=cf, family=fam, expected="pass",
                                     reason="control twin",
                                     confidence=r["confidence"]))

    n_chaos = int(os.environ.get("GEN_CHAOS", "120"))
    for i in range(n_chaos):
        pf = os.path.join("chaos", f"chaos_{i:03d}.gcl")
        with open(os.path.join(OUT, pf), "w") as f:
            f.write(f"// PROBE family=chaos case={i}\n" + chaos_file(rng) + "\n")
        manifest.append(dict(file=pf, family="chaos", expected="fail",
                             reason="random token soup", confidence="high"))

    with open(os.path.join(OUT, "manifest.jsonl"), "w") as f:
        for m in manifest:
            f.write(json.dumps(m) + "\n")
    fams = len(RECIPES)
    probes = sum(1 for m in manifest if m["expected"] == "fail")
    controls = sum(1 for m in manifest if m["expected"] == "pass")
    print(f"{fams} families, {probes} probe files (expected FAIL), "
          f"{controls} control files (expected PASS)")


if __name__ == "__main__":
    main()
