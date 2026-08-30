namespace Graphcal.Static.Interface

/--
The closed set of Static declaration categories that may form typed DAG input
ports. This is shared by bindability validation, namespace resolution, and
import/include composition so those layers cannot drift onto different lists.
-/
inductive StaticInputKind where
  | nominalType
  | dimension
  | index
  deriving DecidableEq, Repr

/--
The three semantic states represented by no annotation, `pub`, and `pub(bind)`.
Only `exportedBindable` promises that a caller may supply a replacement.
-/
inductive Visibility where
  | local
  | exported
  | exportedBindable
  deriving DecidableEq, Repr

/--
Whether a declaration supplies its own definition/default or requires one from
an include/call site's typed static bindings.
-/
inductive Requirement where
  | defaulted
  | required
  deriving DecidableEq, Repr

/--
Semantic role of an input-capable Static declaration after visibility and
required-bindability validation. `fixed` declarations cannot be overridden;
optional inputs retain a local default, and required inputs must be supplied.
-/
inductive StaticRole where
  | fixed
  | optionalInput
  | requiredInput
  deriving DecidableEq, Repr

/-- Construct the validated semantic role represented by visibility and requirement. -/
def staticRoleOf
    (visibility : Visibility)
    (requirement : Requirement) : Option StaticRole :=
  match visibility, requirement with
  | .exportedBindable, .defaulted => some .optionalInput
  | .exportedBindable, .required => some .requiredInput
  | .local, .defaulted | .exported, .defaulted => some .fixed
  | .local, .required | .exported, .required => none

end Graphcal.Static.Interface
