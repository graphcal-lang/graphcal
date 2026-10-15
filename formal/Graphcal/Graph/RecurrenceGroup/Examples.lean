import Graphcal.Graph.RecurrenceGroup.Proofs

/-!
# Recurrence groups: examples

The examples of `internals/recurrence-groups.md`, each as abstract group data
with its checked verdict. Values are scaled to integers so that evaluation
results can be checked by `decide`.
-/

namespace Graphcal.Graph.RecurrenceGroup.Examples

open Graphcal.Graph.RecurrenceGroup
open Graphcal.Graph.RecurrenceGroup.Group

/-- Lean core intentionally leaves `Except` equality to clients. -/
private instance {error value : Type} [DecidableEq error] [DecidableEq value] :
    DecidableEq (Except error value)
  | .error first, .error second =>
      match decEq first second with
      | .isTrue equal => .isTrue (by cases equal; rfl)
      | .isFalse different => .isFalse fun equal => different (Except.error.inj equal)
  | .ok first, .ok second =>
      match decEq first second with
      | .isTrue equal => .isTrue (by cases equal; rfl)
      | .isFalse different => .isFalse fun equal => different (Except.ok.inj equal)
  | .error _, .ok _ => .isFalse fun equal => by contradiction
  | .ok _, .error _ => .isFalse fun equal => by contradiction

/-- `index Step = range(…)`. -/
private def stepAxis : AxisId := 0

/-- `index Phase = { … }`. -/
private def phaseAxis : AxisId := 1

private def owned (definition : Definition) : Member := ⟨.callable, definition⟩

/-- Example evaluation errors. -/
inductive ExampleError where
  | domainViolation
  | unexpectedDependencies
  deriving DecidableEq, Repr

/-! ## Accepted: the motivating `state` / `action` group

```gcl
node action: Dimensionless[Step] = for t: Step { -@k * @state[t] };
node state: Dimensionless[Step] = unfold(Step, @initial_state, |s, prev_t, t| s + @action[prev_t]);
```
-/

/-- Members: `action` (0), `state` (1). -/
def stateAction : Group 2 where
  member := fun x => match x.val with
    | 0 => owned (.forOver stepAxis)
    | _ => owned (.unfoldOver stepAxis)
  reads := [⟨0, 1, .element .stepBinder⟩, ⟨1, 0, .element .previousBinder⟩]

theorem stateAction_accepted : stateAction.check = .ok ⟨[1, 0]⟩ := by decide

/-- `initial_state = 1` and `k = 0.5`, scaled by 16. The dependency lists follow
`Group.elementDeps`: `action[t]` reads `[state[t]]`; `state[t]` reads
`[action[prev_t], state[prev_t]]`. -/
def stateActionEnv : Environment 2 Int ExampleError where
  supplied := fun _ => none
  externallyBlocked := fun _ => false
  compute := fun e values => match e.member.val, e.step, values with
    | 0, _, [state] => .ok (-(state / 2))
    | 1, 0, [] => .ok 16
    | 1, _ + 1, [action, previous] => .ok (previous + action)
    | _, _, _ => .error .unexpectedDependencies

theorem stateAction_state :
    (List.range 4).map (fun k => stateAction.run stateActionEnv [1, 0] 4 ⟨1, k⟩) =
      [.computed 16, .computed 8, .computed 4, .computed 2] := by decide

theorem stateAction_action :
    (List.range 4).map (fun k => stateAction.run stateActionEnv [1, 0] 4 ⟨0, k⟩) =
      [.computed (-8), .computed (-4), .computed (-2), .computed (-1)] := by decide

/-! ## Rejected: a same-step read hidden next to a previous-step read

```gcl
node state: Dimensionless[Step] = unfold(Step, @initial_state,
    |s, prev_t, t| s + @action[prev_t] + 0.1 * @action[t]);
```
-/

def mixedReads : Group 2 where
  member := stateAction.member
  reads := [⟨0, 1, .element .stepBinder⟩, ⟨1, 0, .element .previousBinder⟩,
    ⟨1, 0, .element .stepBinder⟩]

/-- `action → state → action`. -/
theorem mixedReads_rejected : mixedReads.check = .error (.sameStepCycle [0, 1]) := by decide

/-! ## Accepted: a delayed copy for a two-step recurrence

```gcl
node prev_x: Dimensionless[Step] = unfold(Step, @x_before, |ignored, prev_t, t| @x[prev_t]);
node x: Dimensionless[Step] = unfold(Step, @x0, |s, prev_t, t| s + @prev_x[prev_t]);
```
-/

/-- Members: `prev_x` (0), `x` (1). -/
def delayedCopy : Group 2 where
  member := fun _ => owned (.unfoldOver stepAxis)
  reads := [⟨0, 1, .element .previousBinder⟩, ⟨1, 0, .element .previousBinder⟩]

theorem delayedCopy_accepted : delayedCopy.check = .ok ⟨[0, 1]⟩ := by decide

/-- `x_before = 0`, `x0 = 1`. -/
def delayedCopyEnv : Environment 2 Int ExampleError where
  supplied := fun _ => none
  externallyBlocked := fun _ => false
  compute := fun e values => match e.member.val, e.step, values with
    | 0, 0, [] => .ok 0
    | 0, _ + 1, [x, _] => .ok x
    | 1, 0, [] => .ok 1
    | 1, _ + 1, [previousX, previous] => .ok (previous + previousX)
    | _, _, _ => .error .unexpectedDependencies

theorem delayedCopy_fibonacci :
    (List.range 6).map (fun k => delayedCopy.run delayedCopyEnv [0, 1] 6 ⟨1, k⟩) =
      [.computed 1, .computed 1, .computed 2, .computed 3, .computed 5, .computed 8] := by decide

/-! ## Rejected: a member reads itself

```gcl
node x: Dimensionless[Step] = unfold(Step, @x0, |s, prev_t, t| @x[prev_t] + 1.0);
```
-/

def selfReading : Group 1 where
  member := fun _ => owned (.unfoldOver stepAxis)
  reads := [⟨0, 0, .element .previousBinder⟩]

theorem selfReading_rejected :
    selfReading.check = .error (.selfRead ⟨0, 0, .element .previousBinder⟩) := by decide

/-! ## Rejected: reads that are not keyed by the reader's own step binders -/

/-- `state` reads `action` in the given form; `action` reads `@state[t]`. -/
def stateReadsAction (form : ReadForm) : Group 2 where
  member := stateAction.member
  reads := [⟨0, 1, .element .stepBinder⟩, ⟨1, 0, form⟩]

/-- `|s, prev_t, t| s + sum(@action)` -/
theorem wholeValue_rejected :
    (stateReadsAction .whole).check = .error (.notStepKeyed ⟨1, 0, .whole⟩) := by decide

/-- `|s, prev_t, t| s + 1.0 credit`, where `unit credit = (@action[…]) …`. -/
theorem implicit_rejected :
    (stateReadsAction .implicit).check = .error (.notStepKeyed ⟨1, 0, .implicit⟩) := by decide

/-- `|s, prev_t, t| s + sum(for u: Step { @action[u] })`, or a shadowing
`for t: Step` inside the closure. -/
theorem innerBinder_rejected :
    (stateReadsAction (.element .otherLocal)).check =
      .error (.notStepKeyed ⟨1, 0, .element .otherLocal⟩) := by decide

/-- `unfold(Step, @action[nearest_key(Step, 0.0 s)], …)`: no own binder is in
scope in the init. -/
theorem initRead_rejected :
    (stateReadsAction (.element .computed)).check =
      .error (.notStepKeyed ⟨1, 0, .element .computed⟩) := by decide

/-! ## Accepted: the reader's own binder inside a nested `unfold` init

```gcl
node action: Dimensionless[Step] = for t: Step {
    sum(unfold(Substep, @state[t], |acc, prev_u, u| acc * @decay))
};
```
`@state[t]` is keyed by `action`'s own `t`, so it is a same-step read.
-/

theorem nestedInit_accepted :
    stateAction.check = .ok ⟨[1, 0]⟩ ∧
      stateAction.reads.head? = some ⟨0, 1, .element .stepBinder⟩ := by decide

/-! ## Accepted: members with further axes

```gcl
node pop: Dimensionless[Step, Region] = unfold(Step, @pop0,
    |p, prev_t, t| for r: Region { p[r] + @births[prev_t, r] });
node births: Dimensionless[Step, Region] = for t: Step, r: Region { @rate[r] * @pop[t, r] };
```
-/

/-- Members: `pop` (0), `births` (1). -/
def arrayedStock : Group 2 where
  member := fun x => match x.val with
    | 0 => owned (.unfoldOver stepAxis)
    | _ => owned (.forOver stepAxis)
  reads := [⟨0, 1, .element .previousBinder⟩, ⟨1, 0, .element .stepBinder⟩]

theorem arrayedStock_accepted : arrayedStock.check = .ok ⟨[0, 1]⟩ := by decide

/-! ## Rejected: member rules -/

/-- The cycle passes through a node of an included DAG instance. -/
def throughInstance : Group 2 where
  member := fun x => match x.val with
    | 0 => owned (.forOver stepAxis)
    | _ => ⟨.includedInstance, .unfoldOver stepAxis⟩
  reads := stateAction.reads

theorem throughInstance_rejected :
    throughInstance.check = .error (.includedInstanceMember 1) := by decide

/-- `node action: Dimensionless[Step] = todo { @state };` -/
def unfinishedAction : Group 2 where
  member := fun x => match x.val with
    | 0 => owned .unfinished
    | _ => owned (.unfoldOver stepAxis)
  reads := [⟨0, 1, .whole⟩, ⟨1, 0, .element .previousBinder⟩]

theorem unfinishedAction_rejected :
    unfinishedAction.check = .error (.unfinishedMember 0) := by decide

/-- `node gain: Dimensionless = sum(@state);` read by `state`'s closure. -/
def scalarGain : Group 2 where
  member := fun x => match x.val with
    | 0 => owned (.unfoldOver stepAxis)
    | _ => owned .otherFormula
  reads := [⟨0, 1, .whole⟩, ⟨1, 0, .whole⟩]

theorem scalarGain_rejected :
    scalarGain.check = .error (.notRecurrenceShape 1) ∧
      scalarGain.diagnose = some (.recurrenceGroupRejected (.notRecurrenceShape 1)) := by decide

/-- `state` steps along `Step`, `action` along `Phase`. -/
def twoAxes : Group 2 where
  member := fun x => match x.val with
    | 0 => owned (.unfoldOver stepAxis)
    | _ => owned (.forOver phaseAxis)
  reads := [⟨0, 1, .whole⟩, ⟨1, 0, .whole⟩]

theorem twoAxes_rejected : twoAxes.check = .error (.axisMismatch 1) := by decide

/-! ## Plain cycles keep the plain diagnostic -/

/-- `node a = @b + 1.0; node b = @a + 1.0;` -/
def scalarCycle : Group 2 where
  member := fun _ => owned .otherFormula
  reads := [⟨0, 1, .whole⟩, ⟨1, 0, .whole⟩]

theorem scalarCycle_plain : scalarCycle.diagnose = some .dependencyCycle := by decide

/-- `node a = for t: Step { @b[t] }; node b = for t: Step { @a[t] };` -/
def forCycle : Group 2 where
  member := fun _ => owned (.forOver stepAxis)
  reads := [⟨0, 1, .element .stepBinder⟩, ⟨1, 0, .element .stepBinder⟩]

theorem forCycle_plain :
    forCycle.check = .error (.sameStepCycle [0, 1]) ∧
      forCycle.diagnose = some .dependencyCycle := by decide

/-! ## The rule is conservative on one-element axes

`u` is an `unfold` whose closure reads `@f[t]`; `f` is a `for` reading `@u[t]`.
On an axis with one element the closure never runs, so the elements are
acyclic, but the group is rejected because the cycle appears on every longer
axis.
-/

/-- Members: `u` (0), `f` (1). -/
def closureCycle : Group 2 where
  member := fun x => match x.val with
    | 0 => owned (.unfoldOver stepAxis)
    | _ => owned (.forOver stepAxis)
  reads := [⟨0, 1, .element .stepBinder⟩, ⟨1, 0, .element .stepBinder⟩]

theorem closureCycle_rejected : closureCycle.check = .error (.sameStepCycle [0, 1]) := by decide

theorem closureCycle_oneElement_acyclic (e : Element 2) :
    ¬ Relation.TransGen (closureCycle.ElementDepWithin 1) e e := by
  refine not_transGen_self_of_measure (fun e => e.member.val) ?_ e
  rintro ⟨x, k⟩ d ⟨hk, hdep⟩
  have hk0 : k = 0 := by simp only at hk; omega
  subst hk0
  cases hdep with
  | @same r shape _ hr hs _ hrun =>
      simp only [closureCycle, List.mem_cons, List.not_mem_nil, or_false] at hr
      revert hs hrun
      rcases hr with rfl | rfl <;> cases shape <;> decide

theorem closureCycle_twoElements_cyclic :
    ∃ e, Relation.TransGen (closureCycle.ElementDepWithin 2) e e :=
  SameStepCycle.elementCycle
    (sameStepCycle_of_isSameStepCycle (cycle := [0, 1]) (by
      have h := check_error_reported closureCycle_rejected
      exact h.2.2.2)) (Nat.le_refl 2)

/-! ## Outcomes when an element fails

`action` violates its domain at step 3. `state` never reads `action[3]`, so all
its elements are computed, but it reads `action`, so it is reported as a
dependency failure.
-/

def failingEnv : Environment 2 Int ExampleError where
  supplied := fun _ => none
  externallyBlocked := fun _ => false
  compute := fun e values =>
    if e.member.val = 0 ∧ e.step = 3 then .error .domainViolation
    else stateActionEnv.compute e values

private def failingRun : Element 2 → Status Int ExampleError :=
  stateAction.run failingEnv [1, 0] 4

theorem failing_statuses :
    (List.range 4).map (fun k => failingRun ⟨0, k⟩) =
        [.computed (-8), .computed (-4), .computed (-2), .failed .domainViolation] ∧
      (List.range 4).map (fun k => failingRun ⟨1, k⟩) =
        [.computed 16, .computed 8, .computed 4, .computed 2] := by decide

theorem failing_action : MemberFailed failingEnv failingRun 4 0 :=
  ⟨rfl, 3, by decide, .domainViolation, by decide⟩

theorem failing_state : MemberDependencyUnavailable stateAction failingEnv failingRun 4 1 := by
  refine ⟨rfl, ?_, .inr ⟨0, .single ⟨⟨_, List.mem_cons_of_mem _ List.mem_cons_self, rfl, rfl⟩, rfl⟩,
    .inr ⟨3, by decide, .domainViolation, by decide⟩⟩⟩
  rintro ⟨k, hk, error, hfail⟩
  have hcomputed : ∀ k, k < 4 → ∃ v, failingRun ⟨1, k⟩ = .computed v := by
    intro k hk
    match k, hk with
    | 0, _ => exact ⟨16, by decide⟩
    | 1, _ => exact ⟨8, by decide⟩
    | 2, _ => exact ⟨4, by decide⟩
    | 3, _ => exact ⟨2, by decide⟩
  obtain ⟨v, hv⟩ := hcomputed k hk
  rw [hv] at hfail
  cases hfail

/-! ## A supplied param member is not evaluated

```gcl
param state: Dimensionless[Step] = unfold(Step, @initial_state, |s, prev_t, t| s + @action[prev_t]);
```
evaluated with `--param state=…`.
-/

def suppliedEnv : Environment 2 Int ExampleError where
  supplied := fun x => if x.val = 1 then some fun k => [32, 16, 8, 4].getD k 0 else none
  externallyBlocked := fun _ => false
  compute := stateActionEnv.compute

theorem supplied_statuses :
    (List.range 4).map (fun k => stateAction.run suppliedEnv [1, 0] 4 ⟨0, k⟩) =
        [.computed (-16), .computed (-8), .computed (-4), .computed (-2)] ∧
      (List.range 4).map (fun k => stateAction.run suppliedEnv [1, 0] 4 ⟨1, k⟩) =
        [.computed 32, .computed 16, .computed 8, .computed 4] := by decide

end Graphcal.Graph.RecurrenceGroup.Examples
