import Graphcal.Graph.RecurrenceGroup.Model

/-!
# Recurrence groups: reference checker and evaluator

Executable definitions kept independent of the normative specification. The
checker returns the first violation in rule order or the plan; the evaluator
runs a plan step by step and has an explicit `stuck` result for reading an
element that was not evaluated yet.
-/

namespace Graphcal.Graph.RecurrenceGroup

variable {size : Nat}

namespace Group

/-! ## Checker -/

/-- The members `x` reads at the same step, in read order. -/
def sameTargets (g : Group size) (x : Fin size) : List (Fin size) :=
  g.reads.filterMap fun r =>
    if r.source = x ∧ g.access? r = some .same then some r.target else none

/-- `x` is not placed yet and every member it reads at the same step is. -/
def readyB (g : Group size) (placed : List (Fin size)) (x : Fin size) : Bool :=
  decide (x ∉ placed) && (g.sameTargets x).all fun y => decide (y ∈ placed)

/-- Members not placed yet, in source order. -/
def unplaced (placed : List (Fin size)) : List (Fin size) :=
  (List.finRange size).filter fun x => decide (x ∉ placed)

/-- Place members greedily, first ready member in source order first. Returns
the order, or the members that could not be placed. -/
def kahn (g : Group size) : Nat → List (Fin size) → Except (List (Fin size)) (List (Fin size))
  | 0, placed => if unplaced placed = [] then .ok placed else .error (unplaced placed)
  | fuel + 1, placed =>
      if unplaced placed = [] then .ok placed
      else
        match (List.finRange size).find? (g.readyB placed) with
        | some x => g.kahn fuel (placed ++ [x])
        | none => .error (unplaced placed)

/-- The first member, in read order, that `x` reads at the same step among the
stuck members. -/
def stuckNext (g : Group size) (stuck : List (Fin size)) (x : Fin size) : Fin size :=
  ((g.sameTargets x).find? fun y => decide (y ∈ stuck)).getD x

/-- The suffix of a list starting at the first occurrence of `x`. -/
def suffixFrom {α : Type} [DecidableEq α] (x : α) : List α → List α
  | [] => []
  | y :: rest => if y = x then y :: rest else suffixFrom x rest

/-- Follow `stuckNext` until a member repeats, and return the repeated part. -/
def walkCycle (g : Group size) (stuck : List (Fin size)) :
    Nat → List (Fin size) → Fin size → List (Fin size)
  | 0, _, _ => []
  | fuel + 1, visited, x =>
      if x ∈ visited then suffixFrom x visited
      else g.walkCycle stuck fuel (visited ++ [x]) (g.stuckNext stuck x)

/-- The reported same-step cycle: walk from the first stuck member. -/
def extractCycle (g : Group size) (stuck : List (Fin size)) : List (Fin size) :=
  match stuck with
  | [] => []
  | x :: _ => g.walkCycle stuck (size + 1) [] x

/-- `x` is declared by the same DAG as the first member. -/
def ownerMatchesFirst (g : Group size) (x : Fin size) : Bool :=
  decide ((g.member x).owner = (g.member ⟨0, x.pos⟩).owner)

/-- `x` steps along the same axis as the first member. -/
def axisMatchesFirst (g : Group size) (x : Fin size) : Bool :=
  decide ((g.member x).definition.axis? = (g.member ⟨0, x.pos⟩).definition.axis?)

/-- Check a candidate group: R1–R6 in order, then R7 by greedy placement. -/
def check (g : Group size) : Except (Violation size) (Plan size) :=
  match (List.finRange size).find? (fun x => !g.ownerMatchesFirst x) with
  | some x => .error (.mixedOwners x)
  | none =>
  match (List.finRange size).find? (fun x => decide ((g.member x).definition = .unfinished)) with
  | some x => .error (.unfinishedMember x)
  | none =>
  match (List.finRange size).find? (fun x => decide (g.shape? x = none)) with
  | some x => .error (.notRecurrenceShape x)
  | none =>
  match (List.finRange size).find? (fun x => !g.axisMatchesFirst x) with
  | some x => .error (.axisMismatch x)
  | none =>
  match g.reads.find? (fun r => decide (r.source = r.target)) with
  | some r => .error (.selfRead r)
  | none =>
  match g.reads.find? (fun r => decide (g.access? r = none)) with
  | some r => .error (.notStepKeyed r)
  | none =>
  match g.kahn size [] with
  | .ok order => .ok ⟨order⟩
  | .error stuck => .error (.sameStepCycle (g.extractCycle stuck))

/-- Whether some member is an `unfold`. -/
def hasUnfoldMember (g : Group size) : Bool :=
  (List.finRange size).any fun x => decide (g.shape? x = some .unfoldBody)

end Group

/-- The `G001` diagnostic shown for a rejected candidate group. -/
inductive Diagnostic (size : Nat) where
  /-- The plain "cyclic dependency" message. -/
  | dependencyCycle
  /-- "cyclic dependency", explained by the violated recurrence-group rule. -/
  | recurrenceGroupRejected (violation : Violation size)
  deriving DecidableEq, Repr

namespace Group

/-- Without an `unfold` member a group can never be accepted, so its diagnostic
is the plain cycle message. -/
def diagnose (g : Group size) : Option (Diagnostic size) :=
  match g.check with
  | .ok _ => none
  | .error violation =>
      some (if g.hasUnfoldMember then .recurrenceGroupRejected violation else .dependencyCycle)

/-! ## Evaluator -/

/-- The element that read `r` makes its reader read at step `k`, if the read is
evaluated at that step. -/
def readDep (g : Group size) (r : Read size) (k : Nat) : Option (Element size) :=
  match g.shape? r.source with
  | none => none
  | some shape =>
      match shape.access r.form with
      | some .same => if shape.runsBodyAt k then some ⟨r.target, k⟩ else none
      | some .previous =>
          match k with
          | 0 => none
          | k + 1 => some ⟨r.target, k⟩
      | none => none

/-- The previous state an `unfold` member reads at step `k`. -/
def stateDep (g : Group size) (x : Fin size) (k : Nat) : List (Element size) :=
  match g.shape? x, k with
  | some .unfoldBody, k + 1 => [⟨x, k⟩]
  | _, _ => []

/-- The elements that evaluating `e` reads: its reads in read order, then its
previous state. -/
def elementDeps (g : Group size) (e : Element size) : List (Element size) :=
  g.reads.filterMap (fun r => if r.source = e.member then g.readDep r e.step else none)
    ++ g.stateDep e.member e.step

end Group

section Evaluator

variable {Val Err : Type}

/-- Whether an element was evaluated (successfully or not). -/
def Status.isFilled : Status Val Err → Bool
  | .pending => false
  | .stuck => false
  | .computed _ => true
  | .failed _ => true
  | .unavailable => true

/-- The values of `deps`, if all of them were computed. -/
def gatherValues (status : Element size → Status Val Err) : List (Element size) → Option (List Val)
  | [] => some []
  | d :: rest =>
      match status d with
      | .computed value => (gatherValues status rest).map (value :: ·)
      | _ => none

/-- Supplied members start computed; every other element starts pending. -/
def initialStatus (env : Environment size Val Err) : Element size → Status Val Err :=
  fun e =>
    match env.supplied e.member with
    | some values => .computed (values e.step)
    | none => .pending

/-- Replace the status of one element. -/
def update (status : Element size → Status Val Err) (e : Element size) (s : Status Val Err) :
    Element size → Status Val Err :=
  fun d => if d = e then s else status d

/-- Evaluate one element against the current statuses. -/
def Group.process (g : Group size) (env : Environment size Val Err)
    (status : Element size → Status Val Err) (e : Element size) : Status Val Err :=
  match env.supplied e.member with
  | some values => .computed (values e.step)
  | none =>
      if (g.elementDeps e).all fun d => (status d).isFilled then
        if env.externallyBlocked e.member then .unavailable
        else
          match gatherValues status (g.elementDeps e) with
          | some values =>
              match env.compute e values with
              | .ok value => .computed value
              | .error error => .failed error
          | none => .unavailable
      else .stuck

/-- One evaluator step. -/
def Group.evalStep (g : Group size) (env : Environment size Val Err)
    (status : Element size → Status Val Err) (e : Element size) : Element size → Status Val Err :=
  update status e (g.process env status e)

/-- Evaluate a group on an axis with `n` elements, following `order` within
every step. -/
def Group.run (g : Group size) (env : Environment size Val Err) (order : List (Fin size))
    (n : Nat) : Element size → Status Val Err :=
  (schedule order n).foldl (g.evalStep env) (initialStatus env)

end Evaluator

end Graphcal.Graph.RecurrenceGroup
