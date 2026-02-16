# Phase 2: Structs and Multi-Line Nodes

> User-defined struct types. Multi-line node bodies with `let` bindings.

## Goal

Prove that nodes can return structured data and that complex calculations
can be expressed as multi-line bodies with local variables -- eliminating
the "helper column" anti-pattern from spreadsheets.

## Prerequisites

Phase 1 (Dimensions & Units) must be complete. Phase 2 extends the type
system with user-defined structs and extends the syntax with block bodies.

## Design Decisions to Lock

### From [05-algebraic-data-types](../05-algebraic-data-types.md) (structs only)

- [ ] **Struct syntax:** Confirm `type Name { field: Type, ... }`.
      Trailing comma allowed? Required?
- [ ] **Construction syntax:** `TransferResult { dv1, dv2, total_dv: dv1 + dv2 }`.
      Shorthand `{ dv1 }` when field name matches variable name?
- [ ] **Field access:** `@transfer.dv1` -- is `.` always field access, or can it be
      module path? Need to define resolution rules clearly.
      In Phase 2 (single-file), `.` is unambiguous. But the rule must be forward-compatible
      with Phase 4 (multi-file) where `@orbit.transfer.dv1` could mean module path + field.
- [ ] **Nested structs:** Can struct fields be other structs?
      E.g., `type Mission { transfer: TransferResult, budget: Budget }`.
- [ ] **Dimensioned fields:** Confirm fields carry dimensions: `sma: Length`.
      Dimensional arithmetic through field access: `@orbit.sma + 100 km` should type-check.
- [ ] **Struct visibility:** Are struct type declarations file-scoped for now?
      (Multi-file export deferred to Phase 4.)
- [ ] **No tagged unions yet:** Explicitly defer multi-variant types and `match` to post-MVP.
- [ ] **No methods:** Structs have no associated functions. All logic is in free functions or nodes.

### From [02-syntax-design](../02-syntax-design.md) (block bodies)

- [ ] **Block body syntax:** `node x: T = { let a = ...; let b = ...; expr };`
      Confirm the trailing `;` after `}`. Or drop it to match `fn` and `type`?
      This is the semicolon consistency question from the syntax design doc.
- [ ] **`let` binding syntax:** `let name = expr;` or `let name: Type = expr;`?
      Type annotation optional on `let` (inference OK per the "Language for Agents" insight)?
- [ ] **Last expression is the return value:** Confirm Rust-style implicit return
      (last expression in a block, without `;`, is the value).
- [ ] **Conditionals in blocks:** `if cond { a } else { b }` as expressions inside blocks.
      Already in Phase 0 grammar, but now useful in multi-line bodies.

### From [08-scoping](../08-scoping.md)

- [ ] **`let` bindings are local:** `let x = ...` is only visible within the enclosing block.
      Cannot be referenced from other nodes. Does not appear in the DAG.
- [ ] **No shadowing between scopes:** `let G0 = 5` does NOT shadow `@G0`. Confirm.
- [ ] **Shadowing within local scope:** Can a later `let x = ...` shadow an earlier one
      in the same block? Recommendation: no, compile error on duplicate `let` names.

## Syntax Supported in Phase 2

Everything from Phase 1, plus:

```ebnf
// Struct declaration (no trailing ;)
TypeDecl     = "type" IDENT "{" FieldList "}"
FieldList    = (Field ",")* Field ","?        // trailing comma allowed
Field        = IDENT ":" TypeExpr

// Extended node/const with block body
NodeDecl     = "node" IDENT (":" TypeExpr)? "=" (Expr ";" | Block)
Block        = "{" Statement* Expr "}"
Statement    = LetBinding
LetBinding   = "let" IDENT (":" TypeExpr)? "=" Expr ";"

// Struct construction (in expressions)
StructExpr   = IDENT "{" FieldInitList "}"
FieldInitList = (FieldInit ",")* FieldInit ","?
FieldInit    = IDENT (":" Expr)?              // shorthand: { dv1 } == { dv1: dv1 }

// Field access (in expressions)
FieldAccess  = Expr "." IDENT
```

## Implementation Scope

| Component | Description |
| --- | --- |
| **Struct type registry** | Store struct definitions, validate field types |
| **Struct construction** | Type-check field initializers against struct definition |
| **Field access** | Type-check `.field` and resolve to the field's type |
| **Block body evaluator** | Evaluate `let` bindings sequentially, return last expression |
| **Local scope** | Maintain a scope stack for `let` bindings inside blocks |

## Out of Scope

- Tagged unions (multi-variant `type`)
- Pattern matching (`match`)
- Methods on types
- Functions (`fn`)
- Multi-file, imports
- Tables

## Milestone Test

```gcl
type TransferResult {
    dv1: Velocity,
    dv2: Velocity,
    total_dv: Velocity,
    tof: Time,
}

param parking_alt: Length = 200 km;
param target_alt: Length = 35786 km;
const GM_earth = 398600.4418 km^3/s^2;

node transfer: TransferResult = {
    let r1 = @R_earth + @parking_alt;
    let r2 = @R_earth + @target_alt;
    let a = (r1 + r2) / 2.0;

    let v1 = sqrt(@GM_earth / r1);
    let v2 = sqrt(@GM_earth / r2);
    let dv1 = sqrt(2.0 * @GM_earth * r2 / (r1 * (r1 + r2))) - v1;
    let dv2 = v2 - sqrt(2.0 * @GM_earth * r1 / (r2 * (r1 + r2)));

    TransferResult {
        dv1,
        dv2,
        total_dv: dv1 + dv2,
        tof: pi * sqrt(a ^ 3 / @GM_earth),
    }
};

node total_dv: Velocity = @transfer.total_dv;
node tof_hours: Time = @transfer.tof -> hour;
```

### Error cases that must work

```gcl
// error: unknown field
node bad = @transfer.nonexistent;

// error: wrong type in struct construction
type Pair { a: Length, b: Time }
node bad2 = Pair { a: 1.0 kg, b: 2.0 s };
//  error: field `a` expects Length, got Mass
```

## Open Questions

- [ ] Should empty structs be allowed? `type Empty {}`
- [ ] Should struct construction require all fields, or can some be optional
      (with defaults)? Recommendation: require all fields for now.
- [ ] Can a `const` be a struct? E.g., `const earth = Planet { mass: 5.97e24 kg, radius: 6371 km };`
