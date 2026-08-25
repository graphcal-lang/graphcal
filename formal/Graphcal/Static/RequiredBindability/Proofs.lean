import Graphcal.Static.RequiredBindability.Reference
import Graphcal.Static.RequiredBindability.Spec

namespace Graphcal.Static.RequiredBindability

/--
Acceptance is exactly the normative rule—not merely one direction. The reverse
direction is important because a validator that rejected everything would be
sound in the weak sense but unusable.
-/
theorem validate_accepts_iff_wellFormed (decl : InterfaceDecl) :
    validate decl = .ok () ↔ WellFormed decl := by
  cases decl with
  | param requirement =>
      cases requirement <;> simp [validate, WellFormed, Required, Bindable]
  | nominal kind visibility requirement =>
      cases visibility <;> cases requirement <;>
        simp [validate, WellFormed, Required, Bindable]

/-- Every executable rejection identifies a violation that really applies. -/
theorem validate_error_sound
    (decl : InterfaceDecl)
    (violation : Violation)
    (rejected : validate decl = .error violation) :
    violation.AppliesTo decl := by
  cases decl with
  | param requirement =>
      cases requirement <;> cases violation <;>
        simp_all [validate]
  | nominal kind visibility requirement =>
      cases visibility <;> cases requirement <;> cases violation <;>
        simp_all [validate, Violation.AppliesTo]

/--
A required nominal declaration satisfies V002 precisely when it is explicitly
bindable. This theorem is the required row of the state matrix in one line.
-/
theorem required_nominal_wellFormed_iff
    (kind : NominalKind)
    (visibility : Visibility) :
    WellFormed (.nominal kind visibility .required) ↔
      visibility = .exportedBindable := by
  cases visibility <;> simp [WellFormed, Required, Bindable]

/-- Every nominal declaration with a definition is valid under V002. -/
theorem defaulted_nominal_wellFormed
    (kind : NominalKind)
    (visibility : Visibility) :
    WellFormed (.nominal kind visibility .defaulted) := by
  simp [WellFormed, Required]

/-- Both required and defaulted parameters are valid because they are ports. -/
theorem param_wellFormed (requirement : Requirement) :
    WellFormed (.param requirement) := by
  cases requirement <;> simp [WellFormed, Required, Bindable]

end Graphcal.Static.RequiredBindability
