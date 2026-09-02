import Graphcal.Static.TemplateClosure.Model

namespace Graphcal.Static.TemplateClosure

/-- Executable Option A validator, kept independent from the normative specification. -/
def validate : Check → Except Violation Unit
  | ⟨kind, .optionalInput, .templateBody, .defaultDefinition⟩ =>
      .error (.templateBodyDependsOnDefault kind)
  | _ => .ok ()

end Graphcal.Static.TemplateClosure
