namespace Graphcal.Static

/-- The closed set of time scales recognized by Graphcal's type system. -/
inductive TimeScale where
  | utc
  | tai
  | tt
  | tdb
  | et
  | gpst
  | gst
  | bdt
  | qzsst
  deriving DecidableEq, Repr

end Graphcal.Static
