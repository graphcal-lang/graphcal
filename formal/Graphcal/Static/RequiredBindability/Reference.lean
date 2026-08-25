import Graphcal.Static.RequiredBindability.Model

namespace Graphcal.Static.RequiredBindability

/--
The executable reference validator. Its cases mirror the reviewable state
matrix: only required nominal declarations without `pub(bind)` are rejected.
This module intentionally depends only on the semantic model, not on `Spec`.
-/
def validate : InterfaceDecl → Except Violation Unit
  | .param _ => .ok ()
  | .nominal _ _ .defaulted => .ok ()
  | .nominal _ .exportedBindable .required => .ok ()
  | .nominal kind .local .required =>
      .error (.requiredMustBeBindable kind)
  | .nominal kind .exported .required =>
      .error (.requiredMustBeBindable kind)

end Graphcal.Static.RequiredBindability
