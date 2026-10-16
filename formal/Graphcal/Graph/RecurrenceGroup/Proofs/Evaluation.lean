import Graphcal.Graph.RecurrenceGroup.Proofs.Check

/-!
# Recurrence groups: evaluation proofs

For every axis length, a valid order evaluates every element after the
elements it reads; the reference evaluator never reaches `stuck`; a member
reported as succeeded has every element computed; and the final statuses do not
depend on which valid order is used.
-/

namespace Graphcal.Graph.RecurrenceGroup

namespace Group

variable {size : Nat} {g : Group size}

/-! ## Element dependencies -/

theorem access?_eq {r : Read size} {shape : Shape} (hs : g.shape? r.source = some shape) :
    g.access? r = shape.access r.form := by
  simp [access?, hs]

/-- The evaluator's dependency list is the normative element relation. -/
theorem mem_elementDeps {e d : Element size} : d ∈ g.elementDeps e ↔ g.ElementDep e d := by
  obtain ⟨x, k⟩ := e
  simp only [elementDeps, List.mem_append, List.mem_filterMap]
  constructor
  · rintro (⟨r, hr, h⟩ | h)
    · split at h
      · rename_i hsrc
        subst hsrc
        unfold readDep at h
        split at h
        · cases h
        · rename_i shape hs
          split at h
          · rename_i ha
            split at h
            · rename_i hrun
              cases h
              exact .same hr hs ha hrun
            · cases h
          · rename_i ha
            split at h
            · cases h
            · cases h
              exact .previous hr hs ha
          · cases h
      · cases h
    · unfold stateDep at h
      split at h
      · rename_i hs
        rw [List.mem_singleton] at h
        subst h
        exact .previousState hs
      · cases h
  · intro h
    cases h with
    | same hr hs ha hrun =>
        exact .inl ⟨_, hr, by simp [readDep, hs, ha, hrun]⟩
    | previous hr hs ha =>
        exact .inl ⟨_, hr, by simp [readDep, hs, ha]⟩
    | previousState hs =>
        exact .inr (by simp [stateDep, hs])

/-- An element dependency stays within its reader or follows a read. -/
theorem ElementDep.reads_or_self {e d : Element size} (h : g.ElementDep e d) :
    d.member = e.member ∨ g.Reads e.member d.member := by
  cases h with
  | same hr _ _ _ => exact .inr ⟨_, hr, rfl, rfl⟩
  | previous hr _ _ => exact .inr ⟨_, hr, rfl, rfl⟩
  | previousState _ => exact .inl rfl

/-- An element dependency is a same-step read or goes one step back. -/
theorem ElementDep.step_cases {e d : Element size} (h : g.ElementDep e d) :
    (d.step = e.step ∧ g.SameReads e.member d.member) ∨ d.step + 1 = e.step := by
  cases h with
  | same hr hs ha _ => exact .inl ⟨rfl, _, hr, rfl, rfl, by rw [access?_eq hs, ha]⟩
  | previous => exact .inr rfl
  | previousState => exact .inr rfl

/-- Evaluation position decreases along element dependencies. -/
theorem ElementDep.rank_lt {order : List (Fin size)} (hv : g.ValidOrder order)
    {e d : Element size} (h : g.ElementDep e d) :
    d.step * order.length + order.idxOf d.member <
      e.step * order.length + order.idxOf e.member := by
  have hd := List.idxOf_lt_length_of_mem (hv.complete d.member)
  rcases h.step_cases with ⟨hstep, hsame⟩ | hstep
  · rw [hstep]
    have := hv.sameReadsEarlier _ _ hsame
    omega
  · rw [← hstep, Nat.add_mul, Nat.one_mul]
    omega

/-- With a valid order, the element graph is acyclic on every axis. -/
theorem ValidOrder.elementAcyclic {order : List (Fin size)} (hv : g.ValidOrder order)
    (e : Element size) : ¬ Relation.TransGen g.ElementDep e e :=
  not_transGen_self_of_measure (fun e => e.step * order.length + order.idxOf e.member)
    (fun _ _ hab => hab.rank_lt hv) e

/-! ## The schedule -/

theorem mem_schedule {order : List (Fin size)} {n : Nat} {e : Element size} :
    e ∈ schedule order n ↔ e.member ∈ order ∧ e.step < n := by
  obtain ⟨x, k⟩ := e
  simp only [schedule, List.mem_flatMap, List.mem_range, List.mem_map, Element.mk.injEq]
  constructor
  · rintro ⟨k', hk', y, hy, rfl, rfl⟩
    exact ⟨hy, hk'⟩
  · rintro ⟨hx, hk⟩
    exact ⟨k, hk, x, hx, rfl, rfl⟩

theorem nodup_schedule {order : List (Fin size)} (h : order.Nodup) (n : Nat) :
    (schedule order n).Nodup := by
  unfold schedule
  rw [List.Nodup, List.pairwise_flatMap]
  refine ⟨fun k _ => ?_, ?_⟩
  · rw [List.pairwise_map]
    exact h.imp fun hne heq => hne (congrArg Element.member heq)
  · refine (List.nodup_range).imp fun hne a ha b hb heq => hne ?_
    simp only [List.mem_map] at ha hb
    obtain ⟨_, _, rfl⟩ := ha
    obtain ⟨_, _, rfl⟩ := hb
    exact congrArg Element.step heq

theorem schedule_succ (order : List (Fin size)) (n : Nat) :
    schedule order (n + 1) = schedule order n ++ order.map fun x => (⟨x, n⟩ : Element size) := by
  simp [schedule, List.range_succ, List.flatMap_append]

theorem length_schedule (order : List (Fin size)) :
    ∀ n, (schedule order n).length = n * order.length
  | 0 => by simp [schedule]
  | n + 1 => by
      rw [schedule_succ, List.length_append, length_schedule order n, List.length_map,
        Nat.add_mul, Nat.one_mul]

theorem idxOf_map_step (k : Nat) (x : Fin size) :
    ∀ order : List (Fin size),
      (order.map fun y => (⟨y, k⟩ : Element size)).idxOf ⟨x, k⟩ = order.idxOf x
  | [] => rfl
  | y :: rest => by
      simp only [List.map_cons, List.idxOf_cons, idxOf_map_step k x rest]
      by_cases hyx : y = x
      · subst hyx
        simp
      · have h₁ : ((⟨y, k⟩ : Element size) == ⟨x, k⟩) = false :=
          beq_eq_false_iff_ne.2 fun heq => hyx (congrArg Element.member heq)
        have h₂ : (y == x) = false := beq_eq_false_iff_ne.2 hyx
        rw [h₁, h₂]

/-- Position of an element in the schedule. -/
theorem idxOf_schedule {order : List (Fin size)} {e : Element size} (hx : e.member ∈ order) :
    ∀ {n : Nat}, e.step < n →
      (schedule order n).idxOf e = e.step * order.length + order.idxOf e.member
  | 0, hk => absurd hk (Nat.not_lt_zero _)
  | n + 1, hk => by
      obtain ⟨x, k⟩ := e
      rw [schedule_succ, List.idxOf_append]
      by_cases hkn : k < n
      · rw [if_pos (mem_schedule.2 ⟨hx, hkn⟩)]
        exact idxOf_schedule hx hkn
      · have hkeq : k = n := by simp at hk; omega
        subst hkeq
        rw [if_neg fun hmem => hkn (mem_schedule.1 hmem).2, idxOf_map_step, length_schedule]
        dsimp only
        omega

/-- A valid order schedules every element after the elements it reads. -/
theorem ValidOrder.schedule_dep {order : List (Fin size)} (hv : g.ValidOrder order) {n : Nat}
    {e d : Element size} (he : e ∈ schedule order n) (h : g.ElementDep e d) :
    d ∈ schedule order n ∧ (schedule order n).idxOf d < (schedule order n).idxOf e := by
  have hek := (mem_schedule.1 he).2
  have hdk : d.step < n := by
    rcases h.step_cases with ⟨hstep, _⟩ | hstep <;> omega
  refine ⟨mem_schedule.2 ⟨hv.complete _, hdk⟩, ?_⟩
  rw [idxOf_schedule (hv.complete _) hdk, idxOf_schedule (hv.complete _) hek]
  exact h.rank_lt hv

/-! ## The evaluator -/

section Run

variable {Val Err : Type} {env : Environment size Val Err}

theorem foldl_evalStep_other :
    ∀ {l : List (Element size)} {status : Element size → Status Val Err} {e : Element size},
      e ∉ l → (l.foldl (g.evalStep env) status) e = status e
  | [], _, _, _ => rfl
  | d :: rest, status, e, he => by
      have hne : e ≠ d := fun heq => he (heq ▸ List.mem_cons_self)
      have hrest : e ∉ rest := fun hmem => he (List.mem_cons_of_mem d hmem)
      rw [List.foldl_cons, foldl_evalStep_other hrest]
      simp [evalStep, update, hne]

theorem gatherValues_congr {s₁ s₂ : Element size → Status Val Err} :
    ∀ {l : List (Element size)}, (∀ d ∈ l, s₁ d = s₂ d) → gatherValues s₁ l = gatherValues s₂ l
  | [], _ => rfl
  | d :: rest, h => by
      simp only [gatherValues]
      rw [h d List.mem_cons_self, gatherValues_congr fun d' hd' => h d' (List.mem_cons_of_mem _ hd')]

theorem gatherValues_of_computed {s : Element size → Status Val Err} :
    ∀ {l : List (Element size)}, (∀ d ∈ l, ∃ v, s d = .computed v) →
      ∃ values, gatherValues s l = some values
  | [], _ => ⟨[], rfl⟩
  | d :: rest, h => by
      obtain ⟨v, hv⟩ := h d List.mem_cons_self
      obtain ⟨values, hvalues⟩ := gatherValues_of_computed fun d' hd' => h d' (List.mem_cons_of_mem _ hd')
      exact ⟨v :: values, by simp [gatherValues, hv, hvalues]⟩

/-- Evaluating an element looks only at the elements it reads. -/
theorem process_congr {s₁ s₂ : Element size → Status Val Err} {e : Element size}
    (h : ∀ d ∈ g.elementDeps e, s₁ d = s₂ d) : g.process env s₁ e = g.process env s₂ e := by
  unfold process
  rw [all_congr_mem fun d hd => congrArg Status.isFilled (h d hd), gatherValues_congr h]

/-- The final status of every scheduled element is what evaluating it against
the final statuses gives: evaluation solves the element-level recurrence. -/
theorem ValidOrder.run_eq_process {order : List (Fin size)} (hv : g.ValidOrder order) {n : Nat}
    {e : Element size} (he : e ∈ schedule order n) :
    g.run env order n e = g.process env (g.run env order n) e := by
  have hnodup := nodup_schedule hv.nodup n
  obtain ⟨pre, post, hsplit⟩ := List.append_of_mem he
  rw [hsplit] at hnodup
  have hparts := List.nodup_append.1 hnodup
  have hepre : e ∉ pre := fun hmem => hparts.2.2 e hmem e List.mem_cons_self rfl
  have hepost : e ∉ post := (List.nodup_cons.1 hparts.2.1).1
  have hrun : g.run env order n =
      (e :: post).foldl (g.evalStep env) (pre.foldl (g.evalStep env) (initialStatus env)) := by
    unfold run
    rw [hsplit, List.foldl_append]
  have hat : g.run env order n e =
      g.process env (pre.foldl (g.evalStep env) (initialStatus env)) e := by
    rw [hrun, List.foldl_cons, foldl_evalStep_other hepost]
    simp [evalStep, update]
  rw [hat]
  apply process_congr
  intro d hd
  obtain ⟨_, hlt⟩ := hv.schedule_dep he (mem_elementDeps.1 hd)
  rw [hsplit] at hlt
  have hdpre := mem_before_of_idxOf_lt hepre hlt
  rw [hrun, foldl_evalStep_other fun hmem => hparts.2.2 d hdpre d hmem rfl]

/-- The reference evaluator never reads an element before it is evaluated, on
an axis of any length. -/
theorem ValidOrder.run_isFilled {order : List (Fin size)} (hv : g.ValidOrder order) {n : Nat} :
    ∀ e ∈ schedule order n, (g.run env order n e).isFilled = true := by
  suffices ∀ m, ∀ e ∈ schedule order n, (schedule order n).idxOf e = m →
      (g.run env order n e).isFilled = true from fun e he => this _ e he rfl
  intro m
  induction m using Nat.strongRecOn with
  | ind m ih =>
      intro e he hidx
      rw [hv.run_eq_process he]
      unfold process
      split
      · rfl
      · rw [if_pos]
        · split
          · rfl
          · split
            · split <;> rfl
            · rfl
        · rw [List.all_eq_true]
          intro d hd
          obtain ⟨hdmem, hlt⟩ := hv.schedule_dep he (mem_elementDeps.1 hd)
          exact ih _ (hidx ▸ hlt) d hdmem rfl

/-- No scheduled element ends `stuck` or `pending`. -/
theorem ValidOrder.run_not_stuck {order : List (Fin size)} (hv : g.ValidOrder order) {n : Nat}
    {e : Element size} (he : e ∈ schedule order n) :
    g.run env order n e ≠ .stuck ∧ g.run env order n e ≠ .pending := by
  have := hv.run_isFilled (env := env) e he
  constructor <;> intro heq <;> rw [heq] at this <;> cases this

/-- Only an element's own computation produces `failed`. -/
theorem ValidOrder.run_failed {order : List (Fin size)} (hv : g.ValidOrder order) {n : Nat}
    {e : Element size} {error : Err} (h : g.run env order n e = .failed error) :
    ∃ values, env.compute e values = .error error := by
  by_cases he : e ∈ schedule order n
  · rw [hv.run_eq_process he] at h
    unfold process at h
    split at h
    · cases h
    · split at h
      · split at h
        · cases h
        · split at h
          · rename_i values _
            split at h
            · cases h
            · rename_i hc
              cases h
              exact ⟨values, hc⟩
          · cases h
      · cases h
  · unfold run at h
    rw [foldl_evalStep_other he] at h
    unfold initialStatus at h
    split at h <;> cases h

theorem memberOk_of_reads {status : Element size → Status Val Err} {n : Nat} {x y : Fin size}
    (hx : MemberOk g env status n x) (hxy : g.Reads x y) (hy : env.supplied y = none) :
    MemberOk g env status n y :=
  ⟨hy, hx.2.2 y (.single ⟨hxy, hy⟩),
    fun z hz => hx.2.2 z (Relation.TransGen.trans (.single ⟨hxy, hy⟩) hz)⟩

/-- A member reported as succeeded has every element computed. -/
theorem ValidOrder.run_memberOk {order : List (Fin size)} (hv : g.ValidOrder order) {n : Nat} :
    ∀ {x : Fin size} {k : Nat}, MemberOk g env (g.run env order n) n x → k < n →
      ∃ v, g.run env order n ⟨x, k⟩ = .computed v := by
  suffices ∀ m, ∀ (x : Fin size) (k : Nat), (schedule order n).idxOf ⟨x, k⟩ = m →
      MemberOk g env (g.run env order n) n x → k < n →
        ∃ v, g.run env order n ⟨x, k⟩ = .computed v by
    intro x k hx hk
    exact this _ x k rfl hx hk
  intro m
  induction m using Nat.strongRecOn with
  | ind m ih =>
      intro x k hidx hx hk
      have he : (⟨x, k⟩ : Element size) ∈ schedule order n := mem_schedule.2 ⟨hv.complete x, hk⟩
      have hproc := hv.run_eq_process (env := env) he
      have hsup : env.supplied x = none := hx.1
      have hblock : env.externallyBlocked x = false := by
        cases hb : env.externallyBlocked x
        · rfl
        · exact absurd (Or.inl hb) hx.2.1
      have hfilled : (g.elementDeps ⟨x, k⟩).all (fun d => (g.run env order n d).isFilled) = true := by
        rw [List.all_eq_true]
        intro d hd
        exact hv.run_isFilled d (hv.schedule_dep he (mem_elementDeps.1 hd)).1
      obtain ⟨values, hvalues⟩ :
          ∃ values, gatherValues (g.run env order n) (g.elementDeps ⟨x, k⟩) = some values := by
        apply gatherValues_of_computed
        intro d hd
        have hdep := mem_elementDeps.1 hd
        obtain ⟨hdmem, hlt⟩ := hv.schedule_dep he hdep
        obtain ⟨y, j⟩ := d
        cases hy : env.supplied y with
        | some supplied =>
            refine ⟨supplied j, ?_⟩
            rw [hv.run_eq_process hdmem]
            simp [process, hy]
        | none =>
            have hyok : MemberOk g env (g.run env order n) n y := by
              rcases hdep.reads_or_self with hself | hreads
              · have hyx : y = x := hself
                subst hyx
                exact hx
              · exact memberOk_of_reads hx hreads hy
            exact ih _ (hidx ▸ hlt) y j rfl hyok (mem_schedule.1 hdmem).2
      cases hc : env.compute ⟨x, k⟩ values with
      | ok v =>
          exact ⟨v, by rw [hproc]; simp [process, hsup, hfilled, hblock, hvalues, hc]⟩
      | error error =>
          have hfail : g.run env order n ⟨x, k⟩ = .failed error := by
            rw [hproc]
            simp [process, hsup, hfilled, hblock, hvalues, hc]
          exact absurd (Or.inr ⟨k, hk, error, hfail⟩) hx.2.1

/-- If no element fails and every outside input is available, every element of
every member is computed. -/
theorem ValidOrder.run_complete {order : List (Fin size)} (hv : g.ValidOrder order) {n : Nat}
    (hcompute : ∀ e values error, env.compute e values ≠ .error error)
    (hinputs : ∀ x, env.externallyBlocked x = false) (x : Fin size) {k : Nat} (hk : k < n) :
    ∃ v, g.run env order n ⟨x, k⟩ = .computed v := by
  have hnofail : ∀ y, ¬ HasFailure (g.run env order n) n y := by
    rintro y ⟨j, _, error, hj⟩
    obtain ⟨values, hc⟩ := hv.run_failed hj
    exact hcompute _ _ _ hc
  have hnotrouble : ∀ y, ¬ Troubled env (g.run env order n) n y := by
    rintro y (hb | hf)
    · rw [hinputs y] at hb
      cases hb
    · exact hnofail y hf
  cases hsup : env.supplied x with
  | some supplied =>
      refine ⟨supplied k, ?_⟩
      rw [hv.run_eq_process (mem_schedule.2 ⟨hv.complete x, hk⟩)]
      simp [process, hsup]
  | none => exact hv.run_memberOk ⟨hsup, hnotrouble x, fun y _ => hnotrouble y⟩ hk

/-- Every valid order produces the same statuses. The tie-breaking rule of the
plan order affects only which failure is met first. -/
theorem ValidOrder.run_order_independent {o₁ o₂ : List (Fin size)} (h₁ : g.ValidOrder o₁)
    (h₂ : g.ValidOrder o₂) (n : Nat) : g.run env o₁ n = g.run env o₂ n := by
  funext e
  by_cases hk : e.step < n
  · suffices ∀ m, ∀ e ∈ schedule o₁ n, (schedule o₁ n).idxOf e = m →
        g.run env o₁ n e = g.run env o₂ n e from
      this _ e (mem_schedule.2 ⟨h₁.complete _, hk⟩) rfl
    intro m
    induction m using Nat.strongRecOn with
    | ind m ih =>
        intro e he hidx
        have he₂ : e ∈ schedule o₂ n := mem_schedule.2 ⟨h₂.complete _, (mem_schedule.1 he).2⟩
        rw [h₁.run_eq_process he, h₂.run_eq_process he₂]
        apply process_congr
        intro d hd
        obtain ⟨hdmem, hlt⟩ := h₁.schedule_dep he (mem_elementDeps.1 hd)
        exact ih _ (hidx ▸ hlt) d hdmem rfl
  · have h₁' : e ∉ schedule o₁ n := fun h => hk (mem_schedule.1 h).2
    have h₂' : e ∉ schedule o₂ n := fun h => hk (mem_schedule.1 h).2
    unfold run
    rw [foldl_evalStep_other h₁', foldl_evalStep_other h₂']

end Run

end Group

/-! ## Outcomes -/

section Outcomes

variable {size : Nat} {Val Err : Type}

/-- Every evaluated member has exactly one outcome. -/
theorem outcome_cases (g : Group size) (env : Environment size Val Err)
    (status : Element size → Status Val Err) (n : Nat) (x : Fin size)
    (hx : env.supplied x = none) :
    MemberOk g env status n x ∨ MemberFailed env status n x ∨
      MemberDependencyUnavailable g env status n x := by
  by_cases hf : HasFailure status n x
  · exact .inr (.inl ⟨hx, hf⟩)
  · by_cases hdep : env.externallyBlocked x = true ∨ ∃ y, g.DependsOn env x y ∧ Troubled env status n y
    · exact .inr (.inr ⟨hx, hf, hdep⟩)
    · refine .inl ⟨hx, ?_, fun y hy ht => hdep (.inr ⟨y, hy, ht⟩)⟩
      rintro (hb | hfail)
      · exact hdep (.inl hb)
      · exact hf hfail

theorem outcome_exclusive (g : Group size) (env : Environment size Val Err)
    (status : Element size → Status Val Err) (n : Nat) (x : Fin size) :
    ¬ (MemberOk g env status n x ∧ MemberFailed env status n x) ∧
      ¬ (MemberOk g env status n x ∧ MemberDependencyUnavailable g env status n x) ∧
      ¬ (MemberFailed env status n x ∧ MemberDependencyUnavailable g env status n x) := by
  refine ⟨fun ⟨hok, hfail⟩ => hok.2.1 (.inr hfail.2), fun ⟨hok, hdep⟩ => ?_,
    fun ⟨hfail, hdep⟩ => hdep.2.1 hfail.2⟩
  rcases hdep.2.2 with hb | ⟨y, hy, ht⟩
  · exact hok.2.1 (.inl hb)
  · exact hok.2.2 y hy ht

end Outcomes

end Graphcal.Graph.RecurrenceGroup
