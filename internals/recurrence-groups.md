# Recurrence Groups — Specification

This document specifies **recurrence groups**: dependency cycles between
indexed declarations that are acyclic once each declaration is split into its
elements along one axis. It is the normative design for
[#2058](https://github.com/graphcal-lang/graphcal/issues/2058).

The checker is `crates/graphcal-compiler/src/tir/recurrence_group.rs`, called
from `RuntimeSchedule::build` (`tir/schedule.rs`). The evaluator is
`ExecutionFrame::run_group` (`crates/graphcal-eval/src/execution_frame.rs`)
with the element slots of `recurrence_slots.rs`.

The machine-checked version lives in
[`formal/Graphcal/Graph/RecurrenceGroup/`](../formal/Graphcal/Graph/RecurrenceGroup/):

| Lean module | Contents |
|-------------|----------|
| `Model.lean` | Abstract data for one candidate group (§2). |
| `Spec.lean` | The acceptance rule (§3), the reported diagnostic (§6), the element dependency relation, valid evaluation orders and outcomes (§4–§5). |
| `Reference.lean` | Executable reference checker and evaluator. |
| `Proofs/*.lean` | The guarantees of §7. |
| `Examples.lean` | Every example in this document, checked. |

Where this document and the Lean files disagree, that is a bug in one of them.
The Lean names in brackets point to the corresponding definition or theorem.

## 1. Overview

A control loop: the action at a step depends on the state at that step, and the
next state depends on the current state and action.

```gcl
index Step = range(0.0 s, 3.0 s, step: 1.0 s);
param initial_state: Dimensionless = 1.0;
param k: Dimensionless = 0.5;

node action: Dimensionless[Step] = for t: Step { -@k * @state[t] };
node state: Dimensionless[Step] = unfold(Step, @initial_state, |s, prev_t, t| s + @action[prev_t]);
```

At declaration level, `action` reads `state` and `state` reads `action`, which
today is the cycle error `G001`. Element by element there is no cycle:

```text
state[0] → action[0] → state[1] → action[1] → state[2] → action[2] → state[3] → action[3]
```

Under this specification the program is accepted, and evaluation proceeds step
by step:

| Step | `state` | `action` |
|------|---------|----------|
| 0 | 1 (init) | −0.5 |
| 1 | 1 + (−0.5) = 0.5 | −0.25 |
| 2 | 0.25 | −0.125 |
| 3 | 0.125 | −0.0625 |

[`Examples.stateAction_accepted`, `stateAction_state`, `stateAction_action`;
the Lean values are scaled by 16 to stay in integers.]

The rule is deliberately small. It accepts a declaration-level cycle only when
a short, local check shows the element-level graph is acyclic. Every rejection
names one rule and one place in the source.

## 2. Concepts

### Normative model

The dependency graph of a program is the **element-level** graph, in which an
indexed declaration contributes one node per element (`type-system.md`, "DAG
Correspondence"). The declaration-level graph is its projection. A program is
evaluable when the element graph is acyclic.

A recurrence group uses a coarser split: one node per member and step along the
group axis, holding the whole slice at that step. For an accepted group, this
graph is acyclic (§7), and so is the finer one: an edge between two elements of
the same slice would be a same-step self-read, which R5 and R7 exclude.

The checker does not decide element-level acyclicity in general. It accepts a
declaration-level cycle only through the rule in §3, which is sound for every
axis length (§7).

### Candidate groups

The checker builds one runtime dependency graph per callable DAG: the DAG's
params and nodes together with the declarations of the DAG instances it
includes. A **candidate group** is a strongly connected component of that graph
that contains a cycle: two or more declarations, or one declaration that reads
itself. Its declarations are the group's **members**, ordered by source
position. Components without a cycle are scheduled as today.

A program is accepted when every candidate group is accepted. A candidate group
that is not accepted is a `G001` error (§6).

### Steps and elements

An accepted group has one **group axis** `A` that all members step along (R4).
Positions of `A` are **steps** `0, 1, …, n−1`, in the axis order. An
**element** `x[k]` is member `x` at step `k`. For a member with further axes,
such as `T[A, Region]`, the element is the whole slice at step `k`.
[`Element`]

### Reads

A **read** is one way a member's evaluation observes a member's value [`Read`]:

- each occurrence of `@y` in the member's definition, where `y` is a member,
  including occurrences in DAG call arguments, nested closures, and index
  expressions, and
- each **implicit** dependency on a member with no `@y` in the definition. Today
  the only source is a quantity literal whose dynamic unit's scale reads `@y`.

Display conversion targets are not reads: display work runs after the values of
the whole frame exist.

Each read has a **form** [`ReadForm`]:

| Form | Written as |
|------|-----------|
| element access | `@y[k, …]`. Only the leading key `k` is classified. |
| whole value | `@y` without an element access: `sum(@y)`, `count(@y)`, `scan(@y, …)`, `@dag(v: @y)::out`, … |
| implicit | no `@y` in the definition |

### Own binders and the leading key

The **own binders** of a member are the binders of its top-level definition:

- for `for t: A, … { … }`: `t`, the variable of the first binding;
- for `unfold(A, init, |prev_state, prev_t, t| …)`: the step binder `t` and the
  previous-step binder `prev_t`.

They are in scope inside that `for` body or that closure, and never in the
`unfold` init. The leading key of an element access is classified by **binder
identity**, not spelling [`LeadingKey`]:

| Leading key | Class |
|-------------|-------|
| the member's own step binder | `stepBinder` |
| the member's own previous-step binder | `previousBinder` |
| any other local variable: inner `for`, nested `scan`/`unfold` binders, later bindings of a multi-binding `for`, `match` bindings, a variable that shadows an own binder | `otherLocal` |
| anything else: a label, `key(…)`, `Fin` key arithmetic, `nearest_key(…)`, `argmax(…)`, a stored key `@k` | `computed` |

### Step access

A read is **step-keyed** when its access is defined [`Shape.access`]:

| Reader | Read | Access |
|--------|------|--------|
| `for` or `unfold` member | `@y[t, …]` with `t` the own step binder | **same**: `y` at the reader's step |
| `unfold` member | `@y[prev_t, …]` with `prev_t` the own previous-step binder | **previous**: `y` at the reader's previous step |
| any member | anything else | not step-keyed |

A `for` member has no previous-step binder, so only `unfold` members read the
previous step. To lag a `for`-style quantity, use an `unfold` whose state is
ignored, as in the delayed copy under R7.

## 3. Acceptance rule

A candidate group is accepted exactly when R1–R7 hold [`Group.Accepted`]. The
reference checker decides this [`Group.check`, `check_ok_iff_accepted`].

No rule refers to declaration order. Declaring the members, or the references
within them, in a different order never changes whether a group is accepted
[`Relabeling.accepted_iff`; `Examples.stateActionSwapped_accepted`]. Source
order is used only to make diagnostics (§6) and the reference evaluation order
(§4.1) deterministic.

### R1. One owner

All members are declared by the same DAG [`SingleOwner`].

The runtime schedule of a DAG also contains the declarations of the DAG
instances it includes, so a cycle can pass through an instance: the includer
binds an instance param to `@state`, and reads an instance output in `state`.
Such cycles mix owners and are rejected. Recurrence groups across include
boundaries are a possible extension, not part of this rule.
[`Examples.throughInstance_rejected`]

A group entirely inside an included instance has one owner, so a DAG with a
recurrence group of its own can be included.
[`Examples.insideInstance_accepted`]

### R2. Members are finished

No member is an unfinished node, whose formula contains a `todo<T> { … }` hole
[`Finished`].

```gcl
node action: Dimensionless[Step] = todo<Dimensionless[Step]> { @state };   // ✗ R2
node state: Dimensionless[Step] = unfold(Step, @initial_state, |s, prev_t, t| s + @action[prev_t]);
```

A hole's interface lists declarations, not step offsets, so R6 and R7 cannot be
checked. This is today's behavior for cycles through unfinished nodes.
[`Examples.unfinishedAction_rejected`]

Hole interfaces with keyed element reads
([#2099](https://github.com/graphcal-lang/graphcal/issues/2099)) would make R2
redundant. A member whose entire body is a hole has no recurrence shape (R3),
and a hole inside a `for` or `unfold` member is classified through its interface,
whose reads carry step offsets:

```gcl
node action: Dimensionless[Step] = for t: Step { todo<Dimensionless> { @k, @state[t] } };
node state: Dimensionless[Step] = unfold(Step, todo<Dimensionless> { @initial_state }, |s, prev_t, t| todo<Dimensionless> { s, @action[prev_t] });
```

That relaxation only accepts more programs: filling a hole within its interface
removes reads, which never breaks R6 or R7.

### R3. Members are a top-level `for` or `unfold`

Every member's definition (a node formula or a param default) is, after
removing enclosing parentheses and one top-level display conversion
(`… -> km`), exactly one of [`RecurrenceShaped`]:

- `for t: A, … { body }`. A tuple-key marker `(t, …) => body` is allowed.
- `unfold(A, init, |prev_state, prev_t, t| body)`.

A `for` or `unfold` nested in a larger expression does not qualify, and neither
does `scan`, a map literal, or arithmetic on whole values.

```gcl
node state: Dimensionless[Step] = unfold(Step, @initial_state, |s, prev_t, t| s * @gain);
node gain: Dimensionless = sum(@state);   // ✗ R3: not a `for` or `unfold`
```

[`Examples.scalarGain_rejected`]

Both node and param members are allowed. A param member's default is its
definition; §4.4 covers supplied params.

### R4. One axis

All members step along the same axis: the axis of the first `for` binding, or
the `unfold` axis [`CommonAxis`]. Axes are compared by resolved identity: an
imported axis is the same as its declaration, and two different required axes
(`pub(bind) index …`) are different even if one include binds both to the same
concrete axis.

```gcl
node state: Dimensionless[Step] = unfold(Step, @s0, |s, prev_t, t| s + sum(@action));
node action: Dimensionless[Phase] = for p: Phase { @weight[p] * sum(@state) };   // ✗ R4: steps along Phase
```

Both reads are also whole-value reads (R6), but R4 is reported first (§6).

[`Examples.twoAxes_rejected`]

**Consequence.** Every candidate group has a cycle. If every member were a
`for`, every step-keyed read would be a same-step read, and that cycle would
violate R7. So every accepted group has an `unfold` member, and
because `unfold` accepts only coordinate axes (`I036`), the group axis is a
coordinate axis. Named and `Fin` axes never form recurrence groups.
[`Accepted.hasUnfoldMember`, `Accepted.axisCoordinate`]

### R5. No self-reads

No member reads itself, in any form [`NoSelfRead`].

```gcl
node x: Dimensionless[Step] = unfold(Step, @x0, |s, prev_t, t| @x[prev_t] + 1.0);   // ✗ R5: use `s`
```

R5 keeps the `unfold` state binding the only way to read a member's own
previous value, as decided in #1029. It is a canonical-form rule, not a
soundness condition: the evaluation guarantees in §7 do not assume it, and a
same-step self-read is also an R7 violation. Reading oneself through another
member, as in the delayed copy below, is allowed.
[`Examples.selfReading_rejected`]

### R6. Reads are step-keyed

Every read between members is step-keyed (§2) [`StepKeyed`]. Only the leading
key is constrained. Keys in later positions are unrestricted, and any `@z` they
contain is a read of its own.

| In `state`'s closure `\|s, prev_t, t\| …` | Verdict |
|---|---|
| `@action[prev_t]` | previous ✓ |
| `@action[t]` | same ✓ |
| `sum(@action)` | ✗ whole value [`wholeValue_rejected`] |
| `1.0 credit`, where `unit credit = (@action[…]) …` | ✗ implicit [`implicit_rejected`] |
| `sum(for u: Step { @action[u] })` | ✗ other local [`innerBinder_rejected`] |
| `sum(for t: Step { @action[t] })` (shadows `t`) | ✗ other local |
| `@action[nearest_key(Step, 0.0 s)]` | ✗ computed |

The `unfold` init has no own binder in scope, so any member read there is
rejected:

```gcl
node state: Dimensionless[Step] = unfold(Step, @action[nearest_key(Step, 0.0 s)], …);   // ✗ R6
```

[`initRead_rejected`]

A nested `unfold` is different: its init can use the reader's own step binder.

```gcl
node action: Dimensionless[Step] = for t: Step {
    sum(unfold(Substep, @state[t], |acc, prev_u, u| acc * @decay))   // ✓ same-step read
};
```

With `state` as in §1, the group is accepted. [`nestedInit_accepted`]

Members with further axes are allowed, as long as the group axis comes first.
Here every leading key is an own binder, and `r` appears only in the second
position:

```gcl
node pop: Dimensionless[Step, Region] = unfold(Step, @pop0,
    |p, prev_t, t| for r: Region { p[r] + @births[prev_t, r] });
node births: Dimensionless[Step, Region] = for t: Step, r: Region { @rate[r] * @pop[t, r] };
```

[`arrayedStock_accepted`; reference order `[pop, births]`]

### R7. No same-step cycle

The **same-step relation** has `x → y` when **at least one** read from `x` to `y`
is a same-step read. R7 requires this relation to be acyclic
[`SameStepCycle`].

Classification is per read, not per declaration pair. A pair that also has a
previous-step read is still a same-step edge:

```gcl
node action: Dimensionless[Step] = for t: Step { -@k * @state[t] };
node state: Dimensionless[Step] = unfold(Step, @initial_state,
    |s, prev_t, t| s + @action[prev_t] + 0.1 * @action[t]);   // ✗ R7: action → state → action
```

Here `state[t]` needs `action[t]`, which needs `state[t]`.
[`Examples.mixedReads_rejected`]

Previous-step reads never close a cycle. That is how a two-step recurrence
`x[t] = x[t−1] + x[t−2]` is written, with a delayed copy:

```gcl
// prev_x[t] = x[t-1]; the value before the first step is @x_before.
node prev_x: Dimensionless[Step] = unfold(Step, @x_before, |ignored, prev_t, t| @x[prev_t]);
// x[t] = x[t-1] + x[t-2]
node x: Dimensionless[Step] = unfold(Step, @x0, |s, prev_t, t| s + @prev_x[prev_t]);
```

With `x_before = 0` and `x0 = 1`, `x` is `1, 1, 2, 3, 5, 8, …`.
[`Examples.delayedCopy_accepted`, `delayedCopy_fibonacci`]

## 4. Evaluation

### 4.1 Evaluation order within a step

Within each step, members are evaluated in a **valid order**: every member comes
after the members it reads at the same step [`Group.ValidOrder`]. A valid order
exists exactly when R7 holds. Any valid order is conforming: every valid order
gives the same element statuses and values [`ValidOrder.run_order_independent`].

- `state` / `action`: `action` reads `state` at the same step, so the only valid
  order is `[state, action]`.
- Delayed copy: there are no same-step reads, so both orders are valid.

The reference checker returns one particular valid order, so that results are
reproducible: it places members one at a time, each time taking the first member
in source order whose same-step targets are already placed [`Group.PlanOrder`,
`check_ok_planOrder`, `planOrder_unique`]. For the delayed copy this is
`[prev_x, x]`. This tie-break is a determinism choice, not part of the
semantics.

### 4.2 Schedule and element semantics

Evaluation is step-major: for `k = 0, 1, …, n−1`, and within each step for every
member in a valid order, evaluate element `x[k]` [`schedule`].

- `for` member: `x[k]` evaluates the body with `t` bound to the key at step `k`.
- `unfold` member: `x[0]` is the init. For `k ≥ 1`, `x[k]` evaluates the
  closure with `prev_state = x[k−1]`, `prev_t` and `t` bound to the keys at steps
  `k−1` and `k`.

Element `x[k]` reads [`Group.ElementDep`]:

- `y[k]` for each same-step read `x → y`, if `x`'s body runs at step `k` (a
  `for` body at every step, an `unfold` closure from step 1 on);
- `y[k−1]` for each previous-step read, for `k ≥ 1`;
- `x[k−1]`, its own previous state, if `x` is an `unfold` and `k ≥ 1`.

Reads are branch-insensitive: a read anywhere in a body, including an unselected
`if` or `match` arm, counts at every step where the body runs.

### 4.3 The group within the program

The declaration scheduler treats an accepted group as one unit. The unit runs
after every outside declaration that any member reads. Those declarations cannot
read a member, or they would be in the group. Outside declarations that read a
member run after the unit and see the member as an ordinary indexed value.
Reads from members to outside declarations are unrestricted: any key, whole
value, or aggregation.

Each element's domain constraint is checked when the element is committed, so
no other element reads an out-of-domain value. A violation is a failure of that
element. Display conversions are applied after the frame's values exist, as for
any declaration.

### 4.4 Supplied params

A param member supplied by `--param`/`--params-json`, an include binding, or a
DAG call argument is not evaluated. Its supplied value fills all of its elements
before the group runs, and its default, including any reads in it, is ignored.
Acceptance is decided without knowing which params will be supplied. Supplying
one is sound because the rules only get easier to satisfy when a member's reads
disappear [`Environment.supplied`; the §7 theorems hold for any set of
supplied members].

```gcl
param state: Dimensionless[Step] = unfold(Step, @initial_state, |s, prev_t, t| s + @action[prev_t]);
node action: Dimensionless[Step] = for t: Step { -@k * @state[t] };
```

With `state` supplied as `2, 1, 0.5, 0.25`, `action` is `−1, −0.5, −0.25, −0.125`.
[`Examples.supplied_statuses`]

### 4.5 No early reads

The reference evaluator has an explicit internal-error result for reading an
element that was not evaluated yet (`stuck`). For every valid order and every
axis length, it is unreachable [`ValidOrder.run_not_stuck`]. An implementation
must keep the same guard as an internal error. This makes any disagreement
between checker and evaluator, the failure behind #1008, fail loudly instead of
returning a wrong value.

## 5. Failures and outcomes

### 5.1 Element statuses

Each element ends in one of [`Status`, `Group.process`]:

- **computed**;
- **failed**: its own evaluation failed, including a domain violation;
- **unavailable**: its member reads an unavailable outside declaration, or an
  element it reads is failed or unavailable.

Evaluation continues past failures. Each element's status depends only on the
statuses of the elements it reads, so it is the same for every valid order
[`ValidOrder.run_order_independent`]. When the evaluation
policy aborts at the first error, the reported error is the first failed element
the evaluator meets. If several elements of one step fail, which one that is
depends on the valid order used; with the reference order it is deterministic.
This is the only observable effect of choosing among valid orders.

### 5.2 Member outcomes

Values are scheduled per element, but outcomes are reported per declaration,
following today's rule that blocking follows the static, declaration-level
dependency graph. Every member that is not supplied has exactly one outcome
[`outcome_cases`, `outcome_exclusive`]:

| Outcome | Condition |
|---------|-----------|
| **ok** | None of its elements failed, none of its outside inputs is unavailable, and the same holds for every member it depends on (transitively, through reads of members that are not supplied). |
| **failed** | Some element failed. The reported error is the failure at its earliest step. |
| **dependency unavailable** | Otherwise. The reason kind (TODO, blocked, dependency failed) follows today's evaluator. |

Every element of an ok member is computed [`ValidOrder.run_memberOk`]. In
particular, if no element fails and every outside input is available, every
element is computed [`ValidOrder.run_complete`].

**Example.** In the `state` / `action` group, suppose `action` violates its
domain at step 3. `state[3]` reads `action[2]`, not `action[3]`, so all of
`state` is computed:

| Step | `state` | `action` |
|------|---------|----------|
| 0–2 | computed | computed |
| 3 | computed | **failed** |

`action` is **failed**. `state` reads `action`, so it is **dependency
unavailable**, and its computed values are discarded. Reporting per element
would be more precise, but would make one declaration partly available, which
no other part of the language does. [`Examples.failing_statuses`,
`failing_action`, `failing_state`]

## 6. Diagnostics

A rejected candidate group is reported with the existing code `G001`, whose
primary message names the member at the anchor below. The reason depends on
whether the group has an `unfold` member [`Group.diagnose`]:

- **No `unfold` member.** The group can never be accepted (§3, R4
  consequence), so the plain cycle message is shown, exactly as today. Ordinary
  accidental cycles keep their current diagnostic.
  [`diagnose_dependencyCycle`; `Examples.scalarCycle_plain`, `forCycle_plain`]
- **Otherwise.** The diagnostic explains the first violation: rules in order
  R1–R7, members in source order within R1–R4, and reads in source order within
  R5–R6 [`Violation.Reported`, `check_error_reported`, `diagnose_reported`].
  Source order only chooses which of several violations is reported first,
  as it does for today's `G001`; it never decides whether the group is
  rejected.
- **A group of an included DAG.** Its members are declared in another source,
  so a violation there is reported as the plain cycle at the including
  declaration. The included DAG's own check reports the reason.

| Rule | Anchor | Reason (wording illustrative) |
|------|--------|-------------------------------|
| R1 | the first member, in source order, of the checked DAG that reads a member of another DAG | "this cycle reads `y`, declared in another DAG" |
| R2 | the `todo` member | "`x` is unfinished; a recurrence needs formulas to check step offsets" |
| R3 | the member | "`x` is in a cycle with an `unfold`, but is not a top-level `for` or `unfold`" |
| R4 | the member | "`x` steps along `B`, but `first` steps along `A`" |
| R5 | the read | "`x` reads itself; use the `unfold` state binding" |
| R6 | the read (for an implicit read, the quantity literal) | "`sum(@y)` reads all of `y`, which is computed in this recurrence" / "index `y` with this node's own `t` or `prev_t`" |
| R7 | the first same-step read, in source order, from the cycle's first member to its second | "`a → b → a` is a cycle of same-step reads; one read must use `prev_t`" |

The R7 cycle is chosen deterministically: start at the first member, in source
order, that cannot be placed in the reference order. Repeatedly move to the first
member, in read order, that it reads at the same step and that also cannot be
placed. Stop when a member repeats, and report the repeating part
[`Group.extractCycle`, `extractCycle_spec`].

LSP diagnostics are the same diagnostics.

## 7. Guarantees

All theorems are in `formal/Graphcal/Graph/RecurrenceGroup/Proofs/` and hold
for every axis length `n`.

| Guarantee | Theorem |
|-----------|---------|
| The checker accepts exactly the groups satisfying R1–R7. | `Group.check_ok_iff_accepted` |
| Every rejection reports the violation selected in §6, and an R7 rejection lists a real same-step cycle. | `Group.check_error_reported` |
| An accepted group's reference order exists, is unique, and is a valid order. | `check_ok_planOrder`, `planOrder_unique`, `PlanOrder.validOrder` |
| Acceptance does not depend on declaration order. | `Relabeling.accepted_iff` |
| **Soundness:** an accepted group's element graph is acyclic. | `Group.Accepted.elementAcyclic` |
| The evaluator reads exactly the element dependencies of §4.2. | `Group.mem_elementDeps` |
| The evaluator never reads an element before evaluating it. | `ValidOrder.run_not_stuck` |
| An ok member has every element computed. | `ValidOrder.run_memberOk`, `ValidOrder.run_complete` |
| Statuses do not depend on which valid order is used. | `ValidOrder.run_order_independent` |
| Every accepted group has an `unfold` member and steps along a coordinate axis. | `Accepted.hasUnfoldMember`, `Accepted.axisCoordinate` |
| **R7 is exact for the read abstraction:** a same-step cycle is an element cycle on every axis with at least two elements. | `SameStepCycle.elementCycle` |
| A cycle without an `unfold` member gets the plain diagnostic. | `diagnose_dependencyCycle` |

The guarantees are about the abstract data of §2. They transfer to the compiler
only if the compiler produces that data faithfully (§9).

## 8. What the rule does not accept

The rule is sound but not complete. It rejects some groups whose element graph
is acyclic:

- **One-element axes.** A same-step cycle through an `unfold` closure has no
  element cycle when the closure never runs. R7 rejects it anyway, so that
  acceptance does not depend on the axis length. With two or more elements the
  cycle is real. [`Examples.closureCycle_oneElement_acyclic`,
  `closureCycle_twoElements_cyclic`]
- **Branch-dependent reads.** `if coord(t) > 1.0 s { @y[t] } else { … }`
  counts at every step.
- **Computed keys**, such as `Fin` key arithmetic `@y[i + 1]` on named or `Fin`
  axes, or a key search that happens to select an earlier step.
- **Reads of members in an `unfold` init.** An initial value derived from other
  members at the first step, which Vensim supports, must be computed outside
  the group.
- **`scan` members**, **cycles across include boundaries** (R1), and **`todo`
  members** (R2; expression-level holes, #2099, would lift this).

Two related cases are outside the rule for other reasons:

- **Multi-step delays** such as `x[t−2]` cannot be written as a key of a
  coordinate axis. Use a delayed copy (R7).
- **Same-step algebraic loops** have a real element cycle. They need a solver
  (#888).

Each of these is a possible extension, to be decided separately. None of them is
needed for soundness.

## 9. Upstream obligations

The Lean model takes the following data as given. The implementation must
produce it from the same sources the scheduler uses; each item names where it
does.

1. **Candidate groups** are the strongly connected components, containing a
   cycle, of the graph `RuntimeSchedule::build` orders: params and nodes of the
   callable and of its instance closure, with edges from `runtime_deps`
   (`DependencyGraph::cyclic_components`).
2. **Members.** For each member: its owning DAG; its definition shape after
   stripping parentheses (the parser drops them) and one top-level display
   conversion; the resolved axis of a `for` or `unfold`; whether it is a
   `todo` node (`group_facts`).
3. **Reads are complete.** One read per dependency source, in source order:
   every `@y` occurrence that contributes to `runtime_deps`, and every implicit
   dependency (today: one per quantity literal per member its dynamic unit's
   scale reads). Projecting reads to `(source, target)` pairs must give exactly
   `runtime_deps` restricted to the group. A dependency that is merged into an
   already-explained edge must still appear as its own read, because
   classification is per read. `group_facts` walks the HIR for `@y`
   occurrences and dynamic-unit literals, and adds every remaining
   `runtime_deps` edge into the group as an implicit read, so no dependency
   escapes classification.
4. **Leading keys by binder identity.** `IndexArg::Var(id)` is `stepBinder` or
   `previousBinder` when `id` is the member's own binder, and `otherLocal`
   otherwise. `IndexArg::Variant` and `IndexArg::Expr` are `computed`. Spelling
   is never consulted.
5. **Typing.** `unfold` steps only along coordinate axes (`I036`); this is used
   only for the coordinate-axis consequence.
6. **Evaluation.** The evaluator follows the checker's plan
   (`RecurrenceGroupPlan::element_dependencies`) and keeps the `stuck` guard as
   an internal error, both before evaluating an element and when the
   interpreter reads a member (`RecurrenceSlots::computed`). A member is read
   only element by element, through its leading key. Domain constraints are
   checked at element commit, and supplied params are filled before the group
   runs.
7. **Conformance.** `Group.check` and `Group.run` are executable. The Rust
   unit tests of `tir/recurrence_group/tests.rs` mirror `Examples.lean`. A
   follow-up is an exhaustive oracle over small groups, like the existing
   `formal-conformance` oracles. It should cover two or three members, every
   shape, and reads of every form and key, compared against `check_group`.

## 10. Changes from the text of #2058

| #2058 text | This specification |
|------------|-------------------|
| Causality: "edges made only of same-step references are acyclic" | Per read: one same-step read makes a same-step edge (R7). |
| "Accepted only if" rules 1–3, plus a separate self-reference decision | R1–R7 are the whole rule; self-reads are R5, documented as a canonical-form rule. |
| Delayed copy with `\|_, prev_t, t\|` | `_` is not an identifier; the example uses `ignored`. |
| Axes unconstrained | Groups step along coordinate axes, as a proved consequence. |
| Multi-axis members unspecified | Allowed; only the leading key is constrained (R6). |
| Failures unspecified | §5: per-element statuses, per-declaration outcomes, domain checks at commit. |
| Params, `todo`, implicit reads, instances unspecified | Params allowed, supplied params pre-filled (§4.4); `todo` rejected (R2, to be relaxed by #2099); implicit reads rejected (R6); cycles across include boundaries rejected, groups inside an included DAG allowed (R1). |
| "Inside an `unfold` init" | Covered by binder scope: the top-level init has no own binder, while nested inits may use the reader's `t`. |
| "Existing `G001`" vs "specific diagnostics" | `G001` with a typed reason, a fixed priority, and anchors (§6); plain message without an `unfold` member. |
| Evaluation order unspecified within a step | Any valid order (§4.1); statuses are order-independent; the reference order is a deterministic choice. |
| "Group edges map to `Identity` and `Shift(-1)`" | Same-step reads are `Identity`. Previous-step reads go to the previous key of a coordinate axis, which #1567's `Shift(c)` (defined for `Fin`) does not cover. When #1567 lands, its relation restricted to a group must equal `ElementDep`. |

## 11. Related work and follow-ups

- #1567 (element-level dependency analysis): `Group.ElementDep` is the group
  fragment of that relation.
- #1029 (`prev_state` binding): R5 keeps self-references rejected.
- #888 (bounded solve): same-step algebraic loops stay out of scope (R7).
- #2099 (expression-level `todo` holes): independent of this rule; makes R2
  redundant when it lands.
- When implementing, reconcile `docs/*/language/indexes.md` ("Evaluation
  granularity") and the statements in `computation-model.md` and
  `type-system.md` ("DAG Correspondence") with the element-level model of §2.
