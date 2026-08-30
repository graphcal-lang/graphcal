import Graphcal.Static.NamespaceResolution.Proofs.Path

namespace Graphcal.Static.NamespaceResolution

/-- Declaratively valid DAG input targets pass executable category validation. -/
theorem validateInputTarget_complete
    {category : InputBindingCategory}
    {entity : Entity}
    {target : InputBindingTarget}
    (targetMatches : InputTargetMatches category entity target) :
    validateInputTarget category entity = .ok target := by
  cases targetMatches with
  | param => rfl
  | optionalStatic kind id => cases kind <;> rfl
  | requiredStatic kind id => cases kind <;> rfl

/-- Executable DAG input-target success identifies the exact selected category. -/
theorem validateInputTarget_sound
    {category : InputBindingCategory}
    {entity : Entity}
    {target : InputBindingTarget}
    (accepted : validateInputTarget category entity = .ok target) :
    InputTargetMatches category entity target := by
  cases category with
  | unmarked =>
      cases entity with
      | static staticEntity =>
          cases staticEntity with
          | mk id kind => cases kind <;> simp [validateInputTarget] at accepted
      | term termEntity =>
          cases termEntity with
          | mk id kind =>
              cases kind <;> simp [validateInputTarget] at accepted
              all_goals cases accepted; constructor
      | unit unitEntity =>
          cases unitEntity with
          | mk id kind => cases kind <;> simp [validateInputTarget] at accepted
  | marked expected =>
      cases expected <;> cases entity with
      | static staticEntity =>
          cases staticEntity with
          | mk id kind =>
              cases kind with
              | nominalType requirement =>
                  cases requirement <;> simp [validateInputTarget] at accepted
                  all_goals cases accepted; constructor
              | dimension requirement =>
                  cases requirement <;> simp [validateInputTarget] at accepted
                  all_goals cases accepted; constructor
              | index requirement =>
                  cases requirement <;> simp [validateInputTarget] at accepted
                  all_goals cases accepted; constructor
              | genericTypeParam => simp [validateInputTarget] at accepted
              | genericDimParam => simp [validateInputTarget] at accepted
              | genericIndexParam => simp [validateInputTarget] at accepted
              | genericNatParam => simp [validateInputTarget] at accepted
              | timeScale => simp [validateInputTarget] at accepted
      | term termEntity =>
          cases termEntity with
          | mk id kind => cases kind <;> simp [validateInputTarget] at accepted
      | unit unitEntity =>
          cases unitEntity with
          | mk id kind => cases kind <;> simp [validateInputTarget] at accepted

/-- Executable DAG input resolution is complete for the declarative relation. -/
theorem resolveInputBinding_complete
    {environment : Environment}
    {selector : InputBindingSelector}
    {target : InputBindingTarget}
    (resolved : InputBindingResolves environment selector target) :
    resolveInputBinding environment selector = .ok target := by
  obtain ⟨entity, headResolved, targetMatches⟩ := resolved
  have headAccepted := resolveHead_complete headResolved
  have targetAccepted := validateInputTarget_complete targetMatches
  unfold resolveInputBinding
  rw [headAccepted]
  simp only [Bind.bind, Except.bind]
  exact targetAccepted

/-- Executable DAG input resolution is sound for the declarative relation. -/
theorem resolveInputBinding_sound
    {environment : Environment}
    {selector : InputBindingSelector}
    {target : InputBindingTarget}
    (accepted : resolveInputBinding environment selector = .ok target) :
    InputBindingResolves environment selector target := by
  unfold resolveInputBinding at accepted
  cases headResult : resolveHead environment selector.category.space selector.target with
  | error failure =>
      rw [headResult] at accepted
      simp only [Bind.bind, Except.bind] at accepted
      contradiction
  | ok entity =>
      rw [headResult] at accepted
      simp only [Bind.bind, Except.bind] at accepted
      exact ⟨entity, resolveHead_sound headResult, validateInputTarget_sound accepted⟩

/-- DAG input acceptance is exactly the categorized normative relation. -/
theorem resolveInputBinding_accepts_iff
    (environment : Environment)
    (selector : InputBindingSelector)
    (target : InputBindingTarget) :
    resolveInputBinding environment selector = .ok target ↔
      InputBindingResolves environment selector target :=
  ⟨resolveInputBinding_sound, resolveInputBinding_complete⟩


end Graphcal.Static.NamespaceResolution
