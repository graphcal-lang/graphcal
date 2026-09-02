import Graphcal.Static.TemplateClosure.Model

namespace Graphcal.Static.TemplateClosure

/--
Option A's normative closure rule. Only a template-body operation that observes
an optional Static input's concrete default is rejected. Parameter defaults may
observe it because V005 forces an overriding include to reconcile that value.
Fixed declarations have no substitution boundary, and required inputs have no
default definition to observe.
-/
def WellFormed : Check → Prop
  | ⟨_, .optionalInput, .templateBody, .defaultDefinition⟩ => False
  | _ => True

/-- Relate the typed V007 rejection to the exact semantic state that justifies it. -/
def Violation.AppliesTo : Violation → Check → Prop
  | .templateBodyDependsOnDefault expectedKind,
      ⟨actualKind, .optionalInput, .templateBody, .defaultDefinition⟩ =>
        actualKind = expectedKind
  | _, _ => False

/-- Parameter defaults remain valid regardless of their Static dependency. -/
theorem parameter_default_wellFormed
    (kind : StaticInputKind)
    (role : StaticRole)
    (dependency : Dependency) :
    WellFormed ⟨kind, role, .parameterDefault, dependency⟩ := by
  cases role <;> cases dependency <;> simp [WellFormed]

/-- Abstract body uses are parametric for every Static role. -/
theorem abstract_body_wellFormed
    (kind : StaticInputKind)
    (role : StaticRole) :
    WellFormed ⟨kind, role, .templateBody, .abstract⟩ := by
  cases role <;> simp [WellFormed]

/-- The only ill-formed state is a body use of an optional input's default. -/
theorem optional_default_body_not_wellFormed
    (kind : StaticInputKind) :
    ¬ WellFormed ⟨kind, .optionalInput, .templateBody, .defaultDefinition⟩ := by
  simp [WellFormed]

/-- The normative predicate and typed violation relation describe the same rule. -/
theorem wellFormed_iff_no_applicable_violation (check : Check) :
    WellFormed check ↔ ¬ ∃ violation : Violation, violation.AppliesTo check := by
  cases check with
  | mk kind role context dependency =>
      cases kind <;> cases role <;> cases context <;> cases dependency <;>
        simp [WellFormed, Violation.AppliesTo] <;>
        exact ⟨.templateBodyDependsOnDefault _, rfl⟩

end Graphcal.Static.TemplateClosure
