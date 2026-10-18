/-!
# Recurrence groups: model

Abstract data for one *candidate recurrence group*: a strongly connected
component, containing a cycle, of the runtime dependency graph that the
checker builds for one callable DAG (its params with defaults and its nodes,
together with the declarations of the instances it includes).

The data is what the compiler knows after name resolution and type checking.
`internals/recurrence-groups.md` states how the compiler derives every field
from source ("Upstream obligations"); this file fixes only the vocabulary.
-/

namespace Graphcal.Graph.RecurrenceGroup

/-- Resolved identity of an index axis. Two axes are the same axis exactly when
their identities are equal; spelling and import paths play no role. -/
abbrev AxisId := Nat

/-- The kinds of index axes. `unfold` accepts only coordinate axes (`I036`). -/
inductive AxisKind where
  | coordinate
  | named
  | finite
  deriving DecidableEq, Repr

/-- Resolved identity of the DAG that declares a scheduled declaration: the
callable being checked, or a DAG instance it includes. The runtime schedule of
a callable also contains the declarations of the instances it includes, so a
dependency cycle can pass through them. -/
abbrev OwnerId := Nat

/-- The shape of a member's definition: a node formula or a param default,
after removing enclosing parentheses and one top-level display conversion. -/
inductive Definition where
  /-- `for t: A, … { body }` whose first binding iterates the axis `A`. -/
  | forOver (axis : AxisId)
  /-- `unfold(A, init, |prev_state, prev_t, t| body)`. -/
  | unfoldOver (axis : AxisId)
  /-- Any other formula: an aggregation, an arithmetic expression, a map
  literal, `scan`, a `for` or `unfold` nested in a larger expression, … -/
  | otherFormula
  /-- An unfinished node: its formula contains a `todo<T> { … }` hole. -/
  | unfinished
  deriving DecidableEq, Repr

/-- One member of a candidate group. -/
structure Member where
  owner : OwnerId
  definition : Definition
  deriving DecidableEq, Repr

/-- The two definition shapes a recurrence group member may have. -/
inductive Shape where
  | forBody
  | unfoldBody
  deriving DecidableEq, Repr

/-- The recurrence shape of a definition, if it has one. -/
def Definition.shape? : Definition → Option Shape
  | .forOver _ => some .forBody
  | .unfoldOver _ => some .unfoldBody
  | .otherFormula => none
  | .unfinished => none

/-- The axis a recurrence-shaped definition steps along. -/
def Definition.axis? : Definition → Option AxisId
  | .forOver axis => some axis
  | .unfoldOver axis => some axis
  | .otherFormula => none
  | .unfinished => none

/-- How the leading key of an element access `@y[k, …]` is written, decided by
binder identity, never by spelling.

The *own binders* of a member are the binders of its top-level definition:
for `for t: A, …` the variable `t` of the first binding; for
`unfold(A, init, |prev_state, prev_t, t| …)` the closure binders `prev_t` and
`t`. They are in scope only inside that `for` body or that closure, never in
the `unfold` init. -/
inductive LeadingKey where
  /-- The member's own current-step binder (`t`). -/
  | stepBinder
  /-- The member's own previous-step binder (`prev_t`, `unfold` only). -/
  | previousBinder
  /-- Any other local variable: a binder of an inner `for`, of a nested `scan`
  or `unfold`, a later binding of a multi-binding `for`, or a `match` binding. -/
  | otherLocal
  /-- Any other key expression: a label, `key(…)`, `Fin` key arithmetic, a key
  search such as `nearest_key(…)`, `argmax(…)`, or a stored key `@k`. -/
  | computed
  deriving DecidableEq, Repr

/-- How a member's evaluation observes another member's value. -/
inductive ReadForm where
  /-- An element access `@y[k, …]`. Only the leading key is classified; keys
  in later positions are unrestricted. -/
  | element (key : LeadingKey)
  /-- `@y` used without an element access: aggregated, passed whole to a DAG
  call, used as a `scan` source, … -/
  | whole
  /-- A dependency with no `@y` occurrence in the body, such as a quantity
  literal whose dynamic unit's scale reads `@y`. -/
  | implicit
  deriving DecidableEq, Repr

/-- One read of a member by a member, in source order. -/
structure Read (size : Nat) where
  /-- The reading member. -/
  source : Fin size
  /-- The member being read. -/
  target : Fin size
  form : ReadForm
  deriving DecidableEq, Repr

/-- A candidate group: its members in source order and every read between
members in source order. Reads of declarations outside the group are not
listed; they are unrestricted. -/
structure Group (size : Nat) where
  member : Fin size → Member
  reads : List (Read size)

/-- The step offset of a classified read. -/
inductive Access where
  /-- The element at the reader's current step. -/
  | same
  /-- The element at the reader's previous step. -/
  | previous
  deriving DecidableEq, Repr

/-- Classification of a read form inside a member of the given shape. A `for`
member has no previous-step binder, so only `unfold` members read the previous
step. Every other form is unclassified. -/
def Shape.access : Shape → ReadForm → Option Access
  | _, .element .stepBinder => some .same
  | .unfoldBody, .element .previousBinder => some .previous
  | _, _ => none

/-- Whether a member's body is evaluated at step `k`. A `for` body is evaluated
at every step. An `unfold` closure is evaluated from the second step on; the
first step is its init. -/
def Shape.runsBodyAt : Shape → Nat → Bool
  | .forBody, _ => true
  | .unfoldBody, k => k != 0

namespace Group

variable {size : Nat}

/-- The recurrence shape of a member, if it has one. -/
def shape? (g : Group size) (x : Fin size) : Option Shape :=
  (g.member x).definition.shape?

/-- The step access of a read, if the read is step-keyed. -/
def access? (g : Group size) (r : Read size) : Option Access :=
  (g.shape? r.source).bind fun shape => shape.access r.form

end Group

/-- One element of a group: the slice of `member` at position `step` of the
group axis. For a member with further axes the element is the whole slice. -/
structure Element (size : Nat) where
  member : Fin size
  step : Nat
  deriving DecidableEq, Repr

/-- Step-major evaluation of an axis with `n` elements: every step in axis
order, and within a step every member in `order`. -/
def schedule {size : Nat} (order : List (Fin size)) (n : Nat) : List (Element size) :=
  (List.range n).flatMap fun k => order.map fun x => ⟨x, k⟩

/-- Typed reasons a candidate group is rejected. -/
inductive Violation (size : Nat) where
  /-- A member is declared by another DAG than the first member: the cycle
  crosses an include boundary. -/
  | mixedOwners (member : Fin size)
  /-- A member is a `todo` node. -/
  | unfinishedMember (member : Fin size)
  /-- A member's definition is not a top-level `for` or `unfold`. -/
  | notRecurrenceShape (member : Fin size)
  /-- A member steps along a different axis than the first member. -/
  | axisMismatch (member : Fin size)
  /-- A member reads itself; `unfold` provides the previous state instead. -/
  | selfRead (read : Read size)
  /-- A read is not keyed by the reader's own step binders. -/
  | notStepKeyed (read : Read size)
  /-- Same-step reads form a cycle, listed from its first member. -/
  | sameStepCycle (cycle : List (Fin size))
  deriving DecidableEq, Repr

/-- The evaluation plan of an accepted group: the order in which members are
evaluated within every step. -/
structure Plan (size : Nat) where
  order : List (Fin size)
  deriving DecidableEq, Repr

/-- The evaluation status of one element. -/
inductive Status (Val Err : Type) where
  /-- Not evaluated yet. -/
  | pending
  | computed (value : Val)
  /-- Evaluation of this element failed, including a domain-constraint
  violation found when the element is committed. -/
  | failed (error : Err)
  /-- Not evaluated because an input is not available. -/
  | unavailable
  /-- The evaluator tried to read an element that was not evaluated yet. The
  reference evaluator reports this as an internal error; it is unreachable for
  a valid plan. -/
  | stuck
  deriving DecidableEq, Repr

/-- What an evaluation of a group receives from its surroundings. -/
structure Environment (size : Nat) (Val Err : Type) where
  /-- Values supplied for param members (`--param`, an include binding, or a
  DAG call argument), per step. A supplied param's default is not evaluated. -/
  supplied : Fin size → Option (Nat → Val)
  /-- Whether a member reads a declaration outside the group whose value is
  unavailable (failed, unfinished, or itself blocked). -/
  externallyBlocked : Fin size → Bool
  /-- Evaluate one element from the values of its element dependencies, in
  the order of `Group.elementDeps`. -/
  compute : Element size → List Val → Except Err Val

end Graphcal.Graph.RecurrenceGroup
