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

end Graphcal.Static.RequiredBindability
