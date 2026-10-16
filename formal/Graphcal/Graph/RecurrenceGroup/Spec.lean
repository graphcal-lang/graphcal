import Graphcal.Graph.RecurrenceGroup.Model

/-!
# Recurrence groups: normative specification

The acceptance rule (`Group.Accepted`), the diagnostic chosen for a rejected
group (`Violation.Reported`), the element-level dependency relation
(`Group.ElementDep`), the evaluation order (`Group.PlanOrder`, `schedule`),
and the per-declaration outcomes of an evaluation (`MemberOk`, `MemberFailed`,
`MemberDependencyUnavailable`).
-/

namespace Graphcal.Graph.RecurrenceGroup

variable {size : Nat}

/-- `ChainFrom r a [b₁, …, bₖ]` holds when `r a b₁`, `r b₁ b₂`, …, `r bₖ₋₁ bₖ`. -/
def ChainFrom {α : Type} (r : α → α → Prop) : α → List α → Prop
  | _, [] => True
  | a, b :: rest => r a b ∧ ChainFrom r b rest

namespace Group

/-! ## Declaration-level relations -/

/-- `x` reads `y` in any form. These edges are exactly the runtime dependency
edges between members. -/
def Reads (g : Group size) (x y : Fin size) : Prop :=
  ∃ r ∈ g.reads, r.source = x ∧ r.target = y

/-- `x` reads `y` at the same step, through at least one read. Classification
is per read: an edge that also carries previous-step reads is still a
same-step edge. -/
def SameReads (g : Group size) (x y : Fin size) : Prop :=
  ∃ r ∈ g.reads, r.source = x ∧ r.target = y ∧ g.access? r = some .same

/-- The group's reads contain a cycle. Every candidate group has one. -/
def ReadCycle (g : Group size) : Prop :=
  ∃ x, Relation.TransGen g.Reads x x

/-- Same-step reads contain a cycle. -/
def SameStepCycle (g : Group size) : Prop :=
  ∃ x, Relation.TransGen g.SameReads x x

/-- A listed same-step cycle `[x₀, x₁, …, xₖ]`: each member reads the next at
the same step, and `xₖ` reads `x₀`. -/
def IsSameStepCycle (g : Group size) : List (Fin size) → Prop
  | [] => False
  | x :: rest => ChainFrom g.SameReads x (rest ++ [x])

/-! ## Acceptance rules -/

/-- R1: all members are declared by the same DAG. A group inside an included
instance is fine; a cycle across an include boundary is not. -/
def SingleOwner (g : Group size) : Prop :=
  ∀ x y, (g.member x).owner = (g.member y).owner

/-- R2: no member is a `todo` node. -/
def Finished (g : Group size) : Prop :=
  ∀ x, (g.member x).definition ≠ .unfinished

/-- R3: every member is a top-level `for` or `unfold`. -/
def RecurrenceShaped (g : Group size) : Prop :=
  ∀ x, g.shape? x ≠ none

/-- R4: all members step along one axis. -/
def CommonAxis (g : Group size) : Prop :=
  ∀ x y, (g.member x).definition.axis? = (g.member y).definition.axis?

/-- R5: no member reads itself. -/
def NoSelfRead (g : Group size) : Prop :=
  ∀ r ∈ g.reads, r.source ≠ r.target

/-- R6: every read is keyed by the reader's own step binder or previous-step
binder. -/
def StepKeyed (g : Group size) : Prop :=
  ∀ r ∈ g.reads, g.access? r ≠ none

/-- The member-level rules R1–R4. -/
structure MemberRules (g : Group size) : Prop where
  singleOwner : g.SingleOwner
  finished : g.Finished
  recurrenceShaped : g.RecurrenceShaped
  commonAxis : g.CommonAxis

/-- A candidate group is accepted exactly when R1–R7 hold. -/
structure Accepted (g : Group size) : Prop where
  memberRules : g.MemberRules
  noSelfRead : g.NoSelfRead
  stepKeyed : g.StepKeyed
  /-- R7: same-step reads are acyclic. -/
  noSameStepCycle : ¬ g.SameStepCycle

/-- Typing fact from `I036`: `unfold` steps only along coordinate axes. -/
def UnfoldAxesCoordinate (g : Group size) (kind : AxisId → AxisKind) : Prop :=
  ∀ x axis, (g.member x).definition = .unfoldOver axis → kind axis = .coordinate

end Group

/-! ## Diagnostics -/

/-- The single violation reported for a rejected group. Rules are checked in
order R1–R7; within a rule, members are visited in source order and reads in
source order, and the first offender is reported. A same-step cycle is reported
as one listed cycle; which cycle is the deterministic choice of the reference
checker. -/
def Violation.Reported (g : Group size) : Violation size → Prop
  | .mixedOwners x =>
      (g.member x).owner ≠ (g.member ⟨0, x.pos⟩).owner ∧
        ∀ y, y < x → (g.member y).owner = (g.member ⟨0, x.pos⟩).owner
  | .unfinishedMember x =>
      g.SingleOwner ∧ (g.member x).definition = .unfinished ∧
        ∀ y, y < x → (g.member y).definition ≠ .unfinished
  | .notRecurrenceShape x =>
      g.SingleOwner ∧ g.Finished ∧ g.shape? x = none ∧
        ∀ y, y < x → g.shape? y ≠ none
  | .axisMismatch x =>
      g.SingleOwner ∧ g.Finished ∧ g.RecurrenceShaped ∧
        (g.member x).definition.axis? ≠ (g.member ⟨0, x.pos⟩).definition.axis? ∧
        ∀ y, y < x →
          (g.member y).definition.axis? = (g.member ⟨0, x.pos⟩).definition.axis?
  | .selfRead r =>
      g.MemberRules ∧ r.source = r.target ∧
        ∃ before after, g.reads = before ++ r :: after ∧
          ∀ r' ∈ before, r'.source ≠ r'.target
  | .notStepKeyed r =>
      g.MemberRules ∧ g.NoSelfRead ∧ g.access? r = none ∧
        ∃ before after, g.reads = before ++ r :: after ∧
          ∀ r' ∈ before, g.access? r' ≠ none
  | .sameStepCycle cycle =>
      g.MemberRules ∧ g.NoSelfRead ∧ g.StepKeyed ∧ g.IsSameStepCycle cycle

namespace Group

/-! ## Element-level semantics -/

/-- `ElementDep e d`: evaluating element `e` reads element `d`. This element
graph is the normative dependency graph of a group; the declaration-level graph
is its projection. Reads are branch-insensitive: a read anywhere in a body counts
at every step where the body is evaluated. -/
inductive ElementDep (g : Group size) : Element size → Element size → Prop
  /-- A same-step read, at every step where the reader's body runs. -/
  | same {r : Read size} {shape : Shape} {k : Nat} :
      r ∈ g.reads → g.shape? r.source = some shape →
      shape.access r.form = some .same → shape.runsBodyAt k = true →
      ElementDep g ⟨r.source, k⟩ ⟨r.target, k⟩
  /-- A previous-step read, from the second step on. -/
  | previous {r : Read size} {shape : Shape} {k : Nat} :
      r ∈ g.reads → g.shape? r.source = some shape →
      shape.access r.form = some .previous →
      ElementDep g ⟨r.source, k + 1⟩ ⟨r.target, k⟩
  /-- An `unfold` member reads its own previous state. -/
  | previousState {x : Fin size} {k : Nat} :
      g.shape? x = some .unfoldBody → ElementDep g ⟨x, k + 1⟩ ⟨x, k⟩

/-- The element graph of an axis with `n` elements. -/
def ElementDepWithin (g : Group size) (n : Nat) (e d : Element size) : Prop :=
  e.step < n ∧ g.ElementDep e d

/-! ## Evaluation order -/

/-- An order of members is valid when it lists every member once and places
the target of every same-step read before its reader. -/
structure ValidOrder (g : Group size) (order : List (Fin size)) : Prop where
  nodup : order.Nodup
  complete : ∀ x, x ∈ order
  sameReadsEarlier : ∀ x y, g.SameReads x y → order.idxOf y < order.idxOf x

/-- `x` can be placed after `placed`: not yet placed, and every member it reads
at the same step is placed. -/
def Ready (g : Group size) (placed : List (Fin size)) (x : Fin size) : Prop :=
  x ∉ placed ∧ ∀ y, g.SameReads x y → y ∈ placed

/-- Greedy placement: repeatedly place the first member, in source order, that
is ready. This is the reference choice among valid orders; it is not normative,
because every valid order yields the same statuses
(`Group.ValidOrder.run_order_independent`). -/
inductive GreedyFrom (g : Group size) : List (Fin size) → List (Fin size) → Prop
  | complete {placed : List (Fin size)} :
      (∀ x, x ∈ placed) → GreedyFrom g placed []
  | place {placed rest : List (Fin size)} {x : Fin size} :
      g.Ready placed x → (∀ y, g.Ready placed y → x ≤ y) →
      GreedyFrom g (placed ++ [x]) rest → GreedyFrom g placed (x :: rest)

/-- The order the reference checker returns. -/
def PlanOrder (g : Group size) (order : List (Fin size)) : Prop :=
  g.GreedyFrom [] order

/-! ## Declaration order -/

/-- `h` is `g` with its members renumbered by `f` (with inverse `inv`) and its
reads listed in any order: the same program with declarations, or the
references within them, written in a different order. -/
structure Relabeling (g h : Group size) where
  f : Fin size → Fin size
  inv : Fin size → Fin size
  inv_f : ∀ x, inv (f x) = x
  f_inv : ∀ x, f (inv x) = x
  member : ∀ x, h.member (f x) = g.member x
  reads : ∀ r, r ∈ h.reads ↔ ∃ r' ∈ g.reads, r = ⟨f r'.source, f r'.target, r'.form⟩

end Group

/-! ## Outcomes -/

section Outcomes

variable {Val Err : Type}

/-- `x` reaches `y` through reads whose targets are not supplied. -/
def Group.DependsOn (g : Group size) (env : Environment size Val Err) :
    Fin size → Fin size → Prop :=
  Relation.TransGen fun x y => g.Reads x y ∧ env.supplied y = none

/-- Some element of `x` failed. -/
def HasFailure (status : Element size → Status Val Err) (n : Nat) (x : Fin size) :
    Prop :=
  ∃ k, k < n ∧ ∃ error, status ⟨x, k⟩ = .failed error

/-- `x` failed, or one of its inputs outside the group is unavailable. -/
def Troubled (env : Environment size Val Err) (status : Element size → Status Val Err)
    (n : Nat) (x : Fin size) : Prop :=
  env.externallyBlocked x = true ∨ HasFailure status n x

/-- An evaluated member that succeeded: it is not troubled and depends on no
troubled member. -/
def MemberOk (g : Group size) (env : Environment size Val Err)
    (status : Element size → Status Val Err) (n : Nat) (x : Fin size) : Prop :=
  env.supplied x = none ∧ ¬ Troubled env status n x ∧
    ∀ y, g.DependsOn env x y → ¬ Troubled env status n y

/-- An evaluated member that failed. Its reported error is the failure at its
earliest step. -/
def MemberFailed (env : Environment size Val Err) (status : Element size → Status Val Err)
    (n : Nat) (x : Fin size) : Prop :=
  env.supplied x = none ∧ HasFailure status n x

/-- An evaluated member whose own elements did not fail, but which reads an
unavailable outside input or depends on a troubled member. -/
def MemberDependencyUnavailable (g : Group size) (env : Environment size Val Err)
    (status : Element size → Status Val Err) (n : Nat) (x : Fin size) : Prop :=
  env.supplied x = none ∧ ¬ HasFailure status n x ∧
    (env.externallyBlocked x = true ∨ ∃ y, g.DependsOn env x y ∧ Troubled env status n y)

end Outcomes

end Graphcal.Graph.RecurrenceGroup
