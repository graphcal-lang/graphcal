import Graphcal.Graph.RecurrenceGroup.Proofs.Lists
import Graphcal.Graph.RecurrenceGroup.Proofs.Check
import Graphcal.Graph.RecurrenceGroup.Proofs.Evaluation
import Graphcal.Graph.RecurrenceGroup.Proofs.Structure
import Graphcal.Graph.RecurrenceGroup.Proofs.Relabel
import Graphcal.Graph.RecurrenceGroup.Proofs.Monotonicity

/-!
# Recurrence groups: proofs

Headline results, by file:

* `Proofs/Check.lean`: `Group.check_ok_iff_accepted` (the checker accepts
  exactly R1–R7), `Group.check_error_reported` (every rejection is the
  specified violation), `Group.check_ok_planOrder` and `Group.planOrder_unique`
  (the plan is the unique greedy order), `Group.PlanOrder.validOrder`.
* `Proofs/Evaluation.lean`: `Group.mem_elementDeps` (the evaluator reads exactly
  the normative element dependencies), `Group.ValidOrder.run_not_stuck` (it never
  reads an element before evaluating it, for every axis length),
  `Group.ValidOrder.run_memberOk` and `Group.ValidOrder.run_complete` (succeeded
  members are complete), `Group.ValidOrder.run_order_independent` (statuses do
  not depend on the tie-breaking rule), `outcome_cases` and `outcome_exclusive`.
* `Proofs/Structure.lean`: `Group.Accepted.elementAcyclic` (soundness),
  `Group.Accepted.hasUnfoldMember` and `Group.Accepted.axisCoordinate` (groups
  step along coordinate axes), `Group.SameStepCycle.elementCycle` (R7 is exact
  for axes with at least two elements), `Group.diagnose_dependencyCycle`.
* `Proofs/Relabel.lean`: `Group.Relabeling.accepted_iff` (acceptance does not
  depend on declaration order).
* `Proofs/Monotonicity.lean`: `Group.Accepted.of_reads_subset` (removing reads,
  as filling a `todo` hole within its interface does, keeps a group accepted).
-/
