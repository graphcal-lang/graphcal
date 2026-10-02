//! The arms of a label match laid out on its concrete axis.
//!
//! Checking proved that the arms of a match on a key name every entry of the
//! key's axis exactly once. A concrete tree records that proof as a
//! [`LabelDispatch`]: the arm each entry of the axis takes, so evaluation
//! selects an arm by the key's position instead of searching the labels.

use thiserror::Error;

use crate::resolved_name::ResolvedIndexVariant;
use crate::semantic::checked_type::IndexTypeRef;
use crate::semantic::index_axis::IndexAxis;
use crate::semantic::key_value::KeyValue;
use crate::syntax::index_name::IndexEntryKey;
use crate::syntax::non_empty::NonEmpty;

/// The arm a label match takes for each entry of its axis.
///
/// Built only by [`LabelDispatch::try_new`], which admits arms whose labels
/// name every entry of the axis exactly once.
#[derive(Debug, Clone)]
pub struct LabelDispatch {
    axis: IndexAxis,
    /// The arm, by written position, each entry takes, in axis order.
    arms: NonEmpty<usize>,
}

/// Why label arms cannot be laid out on their axis.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LabelDispatchError {
    #[error("a match arm's label names no entry of the axis `{axis}`")]
    LabelOutsideAxis { axis: Box<IndexTypeRef> },
    #[error("match arms take {covered} of the {entries} entries of the axis `{axis}`")]
    Coverage {
        axis: Box<IndexTypeRef>,
        covered: usize,
        entries: usize,
    },
}

impl LabelDispatch {
    /// Lay out arms whose labels are `labels`, in written order, on `axis`.
    ///
    /// # Errors
    ///
    /// Returns a [`LabelDispatchError`] when a label names no entry of the
    /// axis or the labels do not name every entry exactly once.
    pub fn try_new(
        axis: IndexAxis,
        labels: &[&ResolvedIndexVariant],
    ) -> Result<Self, LabelDispatchError> {
        let index = || Box::new(axis.index().clone());
        let mut arms = vec![None; axis.len()];
        for (arm, label) in labels.iter().enumerate() {
            let position = (axis.index().declared_resolved() == Some(label.index()))
                .then(|| axis.position(&IndexEntryKey::named(label.variant().clone())))
                .flatten()
                .ok_or_else(|| LabelDispatchError::LabelOutsideAxis { axis: index() })?;
            arms[position].get_or_insert(arm);
        }
        let covered = arms.iter().flatten().count();
        let arms = arms
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .filter(|_| covered == labels.len())
            .and_then(|arms| NonEmpty::try_from_vec(arms).ok())
            .ok_or_else(|| LabelDispatchError::Coverage {
                axis: index(),
                covered,
                entries: axis.len(),
            })?;
        Ok(Self { axis, arms })
    }

    /// The axis the arms are laid out on.
    #[must_use]
    pub const fn axis(&self) -> &IndexAxis {
        &self.axis
    }

    /// The written position of the arm `key` takes; `None` unless `key` is an
    /// entry of this dispatch's axis.
    #[must_use]
    pub fn arm(&self, key: &KeyValue) -> Option<usize> {
        self.axis
            .matches(key.axis())
            .then(|| self.arms.as_slice()[key.position()])
    }
}

#[cfg(test)]
mod tests {
    use crate::dag_id::DagId;
    use crate::resolved_name::{ResolvedIndexName, ResolvedIndexVariant};
    use crate::semantic::index_axis::IndexAxis;
    use crate::semantic::index_def::FiniteIndex;
    use crate::semantic::key_value::KeyValue;
    use crate::syntax::index_name::{IndexName, IndexVariantName};

    use super::{LabelDispatch, LabelDispatchError};

    fn owner() -> DagId {
        DagId::root_in_package("label-dispatch-tests", "main")
    }

    fn phase() -> IndexAxis {
        IndexAxis::named_for_test(owner(), "Phase", &["Launch", "Cruise", "Landing"])
    }

    fn label(index: &str, variant: &str) -> ResolvedIndexVariant {
        ResolvedIndexVariant::new(
            ResolvedIndexName::for_test(owner(), IndexName::expect_valid(index)),
            IndexVariantName::expect_valid(variant),
        )
    }

    #[test]
    fn keys_select_the_arm_naming_their_entry() {
        let labels = [
            label("Phase", "Cruise"),
            label("Phase", "Landing"),
            label("Phase", "Launch"),
        ];
        let dispatch = LabelDispatch::try_new(phase(), &labels.iter().collect::<Vec<_>>()).unwrap();
        let arm = |position| dispatch.arm(&KeyValue::at(phase(), position).unwrap());
        assert_eq!([arm(0), arm(1), arm(2)], [Some(2), Some(0), Some(1)]);
        let fin = IndexAxis::finite(FiniteIndex::try_from_u64(3).unwrap()).unwrap();
        assert_eq!(dispatch.arm(&KeyValue::at(fin, 0).unwrap()), None);
        assert_eq!(dispatch.axis(), &phase());
    }

    #[test]
    fn arms_must_name_every_entry_once() {
        let launch = label("Phase", "Launch");
        let cruise = label("Phase", "Cruise");
        let landing = label("Phase", "Landing");
        assert!(matches!(
            LabelDispatch::try_new(phase(), &[&launch, &cruise]).unwrap_err(),
            LabelDispatchError::Coverage {
                covered: 2,
                entries: 3,
                ..
            }
        ));
        assert!(matches!(
            LabelDispatch::try_new(phase(), &[&launch, &cruise, &landing, &cruise]).unwrap_err(),
            LabelDispatchError::Coverage {
                covered: 3,
                entries: 3,
                ..
            }
        ));
        let other = label("Mode", "Launch");
        assert!(matches!(
            LabelDispatch::try_new(phase(), &[&launch, &cruise, &other]).unwrap_err(),
            LabelDispatchError::LabelOutsideAxis { .. }
        ));
        let unknown = label("Phase", "Orbit");
        assert!(matches!(
            LabelDispatch::try_new(phase(), &[&unknown]).unwrap_err(),
            LabelDispatchError::LabelOutsideAxis { .. }
        ));
    }
}
