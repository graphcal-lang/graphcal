import Graphcal.Static.TemplateClosure.Reference
import Graphcal.Static.TemplateClosure.Spec

namespace Graphcal.Static.TemplateClosure

/-- The executable validator accepts exactly the normative closure predicate. -/
theorem validate_accepts_iff_wellFormed (check : Check) :
    validate check = .ok () ↔ WellFormed check := by
  cases check with
  | mk kind role context dependency =>
      cases kind <;> cases role <;> cases context <;> cases dependency <;>
        simp [validate, WellFormed]

/-- Every executable rejection reports the applicable typed violation. -/
theorem validate_error_sound
    (check : Check)
    (violation : Violation)
    (rejected : validate check = .error violation) :
    violation.AppliesTo check := by
  cases check with
  | mk kind role context dependency =>
      cases kind <;> cases role <;> cases context <;> cases dependency <;>
        cases violation <;> simp_all [validate, Violation.AppliesTo]

end Graphcal.Static.TemplateClosure
