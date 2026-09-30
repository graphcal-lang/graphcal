//! Runtime values carried together with their presentation.
//!
//! A [`Presented`] value is a value and its presentation in one tree, so the
//! two can never disagree about shape: a struct whose fields are presented
//! separately is a `StructValue` of presented fields, an indexed value whose
//! entries are presented separately an `IndexedValue` of presented entries
//! (over the value's own axis), and any other value is whole, with at most
//! one leaf presentation for all of its leaves. The leaf presentation of a
//! whole value presents leaves of its own kind: construction checks this
//! once, and every other operation keeps it.
//!
//! A value is [pending](EvaluatedRuntimeValue) while a display unit it
//! requests still waits for its owner's frame, and [resolved](ResolvedValue)
//! once every request was computed.

use std::borrow::Cow;
use std::collections::HashMap;

use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::syntax::type_name::FieldName;

use crate::invariant::Invariant;
use crate::presentation_evidence::{
    LeafKind, PendingLeaf, PendingQuantityDisplay, PresentationLeaf, QuantityDisplay, ResolvedLeaf,
};
use crate::runtime_value::{IndexedValue, KeyValue, RuntimeValue, StructValue};
use graphcal_compiler::syntax::index_name::IndexEntryKey;

/// A runtime value together with its presentation, in the value's shape.
#[derive(Debug)]
pub struct Presented<L>(Node<L>);

#[derive(Debug)]
enum Node<L> {
    /// A value whose every leaf is presented by `leaf`, or not at all.
    Whole {
        value: RuntimeValue,
        leaf: Option<L>,
    },
    /// A struct value whose fields are presented separately.
    Struct(StructValue<Presented<L>>),
    /// An indexed value whose entries are presented separately.
    Indexed(IndexedValue<Presented<L>>),
}

/// A borrowed view of a [`Presented`] value's outermost level.
#[derive(Debug)]
pub enum PresentedView<'a, L> {
    /// A value whose every leaf is presented by `leaf`, or not at all; every
    /// leaf of the value is of the leaf's [kind](PresentationLeaf::kind).
    Whole {
        value: &'a RuntimeValue,
        leaf: Option<&'a L>,
    },
    /// A struct value whose fields are presented separately.
    Struct(&'a StructValue<Presented<L>>),
    /// An indexed value whose entries are presented separately.
    Indexed(&'a IndexedValue<Presented<L>>),
}

impl<L> Clone for PresentedView<'_, L> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<L> Copy for PresentedView<'_, L> {}

/// A value evaluated by the interpreter, whose display units may still be
/// pending.
pub type EvaluatedRuntimeValue = Presented<PendingLeaf>;

/// A value whose every display unit was computed.
pub type ResolvedValue = Presented<ResolvedLeaf>;

/// The presented values of evaluated declarations, kept only for values with
/// a presentation. A frame holds every value in its value map too, for the
/// computations that need no presentation.
pub type PresentedMap<L> = HashMap<ResolvedDeclName, Presented<L>>;

/// Pending presented values of evaluated declarations.
pub type PendingPresentedMap = PresentedMap<PendingLeaf>;

/// Resolved presented values of evaluated declarations.
pub type ResolvedPresentedMap = PresentedMap<ResolvedLeaf>;

impl<L: Clone> Clone for Presented<L> {
    fn clone(&self) -> Self {
        #[cfg(any(test, feature = "test-internals"))]
        crate::pipeline_metrics::record(
            crate::pipeline_metrics::Event::PresentationEvidenceCopyNode,
        );
        Self(match &self.0 {
            Node::Whole { value, leaf } => Node::Whole {
                value: value.clone(),
                leaf: leaf.clone(),
            },
            Node::Struct(fields) => Node::Struct(fields.clone()),
            Node::Indexed(entries) => Node::Indexed(entries.clone()),
        })
    }
}

/// Whether every scalar leaf of `value` is a leaf of `kind`.
fn leaves_are(value: &RuntimeValue, kind: LeafKind) -> bool {
    match (value, kind) {
        (RuntimeValue::Indexed(entries), _) => {
            entries.values().iter().all(|entry| leaves_are(entry, kind))
        }
        (RuntimeValue::Quantity(_) | RuntimeValue::Complex(_), LeafKind::Quantity)
        | (RuntimeValue::Datetime(_), LeafKind::Datetime) => true,
        (
            RuntimeValue::Quantity(_)
            | RuntimeValue::Complex(_)
            | RuntimeValue::Datetime(_)
            | RuntimeValue::Bool(_)
            | RuntimeValue::Int(_)
            | RuntimeValue::Key(_)
            | RuntimeValue::Struct(_),
            _,
        ) => false,
    }
}

impl<L> Presented<L> {
    /// A value without a presentation.
    #[must_use]
    pub const fn plain(value: RuntimeValue) -> Self {
        Self(Node::Whole { value, leaf: None })
    }

    /// A value whose every leaf is presented by `leaf`.
    ///
    /// # Errors
    ///
    /// Returns an [`Invariant`] when a leaf of `value` is not of the leaf's
    /// kind: the checker admits a display unit only on quantities and a
    /// display time zone only on datetimes.
    pub fn with_leaf(value: RuntimeValue, leaf: L) -> Result<Self, Invariant>
    where
        L: PresentationLeaf,
    {
        let kind = leaf.kind();
        if leaves_are(&value, kind) {
            Ok(Self(Node::Whole {
                value,
                leaf: Some(leaf),
            }))
        } else {
            Err(Invariant::violated(format_args!(
                "a {kind:?} presentation reached {}",
                value.describe()
            )))
        }
    }

    /// A struct value from its presented fields.
    #[must_use]
    pub(crate) fn from_struct(fields: StructValue<Self>) -> Self {
        if fields.fields().all(|(_, field)| field.is_plain()) {
            Self::plain(RuntimeValue::Struct(fields.map(Self::into_value)))
        } else {
            Self(Node::Struct(fields))
        }
    }

    /// An indexed value from its presented entries.
    #[must_use]
    pub(crate) fn from_indexed(entries: IndexedValue<Self>) -> Self {
        if entries.values().iter().all(Self::is_plain) {
            Self::plain(RuntimeValue::Indexed(entries.map(Self::into_value)))
        } else {
            Self(Node::Indexed(entries))
        }
    }

    /// Whether no leaf of the value is presented.
    #[must_use]
    pub(crate) const fn is_plain(&self) -> bool {
        matches!(self.0, Node::Whole { leaf: None, .. })
    }

    /// The outermost level of the value, borrowed.
    #[must_use]
    pub const fn view(&self) -> PresentedView<'_, L> {
        match &self.0 {
            Node::Whole { value, leaf } => PresentedView::Whole {
                value,
                leaf: leaf.as_ref(),
            },
            Node::Struct(fields) => PresentedView::Struct(fields),
            Node::Indexed(entries) => PresentedView::Indexed(entries),
        }
    }

    /// The runtime value, without its presentation.
    #[must_use]
    pub(crate) fn into_value(self) -> RuntimeValue {
        match self.0 {
            Node::Whole { value, .. } => value,
            Node::Struct(fields) => RuntimeValue::Struct(fields.map(Self::into_value)),
            Node::Indexed(entries) => RuntimeValue::Indexed(entries.map(Self::into_value)),
        }
    }

    /// The runtime value, borrowed when the value is whole.
    #[must_use]
    pub fn value(&self) -> Cow<'_, RuntimeValue>
    where
        L: Clone,
    {
        match &self.0 {
            Node::Whole { value, .. } => Cow::Borrowed(value),
            Node::Struct(_) | Node::Indexed(_) => Cow::Owned(self.clone().into_value()),
        }
    }

    /// The presented fields of a struct value; `None` for any other value.
    #[must_use]
    pub(crate) fn into_fields(self) -> Option<StructValue<Self>>
    where
        L: Clone,
    {
        match self.0 {
            Node::Whole {
                value: RuntimeValue::Struct(fields),
                leaf,
            } => Some(fields.map(|value| {
                Self(Node::Whole {
                    value,
                    leaf: leaf.clone(),
                })
            })),
            Node::Struct(fields) => Some(fields),
            Node::Whole { .. } | Node::Indexed(_) => None,
        }
    }

    /// The presented entries of an indexed value; `None` for any other value.
    #[must_use]
    pub(crate) fn into_entries(self) -> Option<IndexedValue<Self>>
    where
        L: Clone,
    {
        match self.0 {
            Node::Whole {
                value: RuntimeValue::Indexed(entries),
                leaf,
            } => Some(entries.map(|value| {
                Self(Node::Whole {
                    value,
                    leaf: leaf.clone(),
                })
            })),
            Node::Indexed(entries) => Some(entries),
            Node::Whole { .. } | Node::Struct(_) => None,
        }
    }

    /// `value`, a value of this value's type, presented as this value is.
    ///
    /// A struct presentation presents a value of the same constructor field
    /// by field. A value of another constructor of the same union stays
    /// plain: this presentation describes none of its fields.
    ///
    /// # Errors
    ///
    /// Returns an [`Invariant`] when `value` is not of this value's type.
    pub(crate) fn present_alike(&self, value: RuntimeValue) -> Result<Self, Invariant>
    where
        L: PresentationLeaf,
    {
        match (&self.0, value) {
            (Node::Whole { leaf: None, .. }, value) => Ok(Self::plain(value)),
            (
                Node::Whole {
                    leaf: Some(leaf), ..
                },
                value,
            ) => Self::with_leaf(value, leaf.clone()),
            (Node::Struct(template), RuntimeValue::Struct(fields))
                if template.type_name() == fields.type_name()
                    && template.generic_args() == fields.generic_args() =>
            {
                if template.constructor() != fields.constructor() {
                    return Ok(Self::plain(RuntimeValue::Struct(fields)));
                }
                fields
                    .try_map(|name, field| {
                        template
                            .field(name)
                            .ok_or_else(|| {
                                Invariant::violated(format_args!(
                                    "constructor `{}` lost its field `{name}`",
                                    template.constructor()
                                ))
                            })?
                            .present_alike(field)
                    })
                    .map(Self::from_struct)
            }
            (Node::Indexed(template), RuntimeValue::Indexed(entries))
                if template.axis().matches(entries.axis()) =>
            {
                // Both walk the same axis, so each key finds its entry.
                entries
                    .try_map(|key, entry| {
                        template
                            .get(key)
                            .ok_or_else(|| {
                                Invariant::violated(format_args!(
                                    "axis `{}` lost its key `{key}`",
                                    template.index()
                                ))
                            })?
                            .present_alike(entry)
                    })
                    .map(Self::from_indexed)
            }
            (_, value) => Err(Invariant::violated(format_args!(
                "a presentation of another type was applied to {}",
                value.describe()
            ))),
        }
    }

    /// This value, or, when it has no presentation, the value presented as
    /// `initial` is: an unannotated recurrence step keeps the authored
    /// initial display.
    ///
    /// # Errors
    ///
    /// Returns an [`Invariant`] when `initial` is not of this value's type.
    pub(crate) fn with_default_presentation(self, initial: &Self) -> Result<Self, Invariant>
    where
        L: PresentationLeaf,
    {
        if self.is_plain() {
            initial.present_alike(self.into_value())
        } else {
            Ok(self)
        }
    }

    /// Replace every leaf presentation.
    fn try_map_leaves<M, E>(
        self,
        leaf: &mut impl FnMut(L) -> Result<M, E>,
    ) -> Result<Presented<M>, E> {
        Ok(Presented(match self.0 {
            Node::Whole { value, leaf: None } => Node::Whole { value, leaf: None },
            Node::Whole {
                value,
                leaf: Some(presented),
            } => Node::Whole {
                value,
                leaf: Some(leaf(presented)?),
            },
            Node::Struct(fields) => {
                Node::Struct(fields.try_map(|_, field| field.try_map_leaves(leaf))?)
            }
            Node::Indexed(entries) => {
                Node::Indexed(entries.try_map(|_, entry| entry.try_map_leaves(leaf))?)
            }
        }))
    }

    /// Active retention metric: presentation nodes, excluding plain values.
    #[cfg(any(test, feature = "test-internals"))]
    #[must_use]
    pub fn retained_nodes(&self) -> usize {
        match &self.0 {
            Node::Whole { leaf: None, .. } => 0,
            Node::Whole { leaf: Some(_), .. } => 1,
            Node::Struct(fields) => fields
                .fields()
                .map(|(_, field)| field.retained_nodes())
                .fold(1, usize::saturating_add),
            Node::Indexed(entries) => entries
                .values()
                .iter()
                .map(Self::retained_nodes)
                .fold(1, usize::saturating_add),
        }
    }
}

impl Presented<PendingLeaf> {
    /// Replace each pending quantity display, keeping every leaf's kind.
    pub(crate) fn try_map_quantity_displays<E>(
        self,
        mut display: impl FnMut(PendingQuantityDisplay) -> Result<PendingQuantityDisplay, E>,
    ) -> Result<Self, E> {
        self.try_map_leaves(&mut |leaf| match leaf {
            PendingLeaf::Quantity(pending) => display(pending).map(PendingLeaf::Quantity),
            PendingLeaf::Datetime(timezone) => Ok(PendingLeaf::Datetime(timezone)),
        })
    }

    /// Resolve each pending quantity display, keeping every leaf's kind.
    pub(crate) fn try_resolve<E>(
        self,
        mut display: impl FnMut(PendingQuantityDisplay) -> Result<QuantityDisplay, E>,
    ) -> Result<ResolvedValue, E> {
        self.try_map_leaves(&mut |leaf| match leaf {
            PendingLeaf::Quantity(pending) => display(pending).map(ResolvedLeaf::Quantity),
            PendingLeaf::Datetime(timezone) => Ok(ResolvedLeaf::Datetime(timezone)),
        })
    }
}

/// A borrowed part of a presented value, for selecting entries without
/// copying their siblings.
#[derive(Debug)]
pub struct PresentedRef<'a, L>(RefNode<'a, L>);

#[derive(Debug)]
enum RefNode<'a, L> {
    /// A part of a whole value, presented by the whole value's leaf.
    Whole {
        value: &'a RuntimeValue,
        leaf: Option<&'a L>,
    },
    Presented(&'a Presented<L>),
}

impl<L> Clone for PresentedRef<'_, L> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<L> Copy for PresentedRef<'_, L> {}

impl<L> Clone for RefNode<'_, L> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<L> Copy for RefNode<'_, L> {}

/// The borrowed entries of an indexed part of a presented value.
#[derive(Debug)]
pub struct EntriesRef<'a, L>(EntriesNode<'a, L>);

#[derive(Debug)]
enum EntriesNode<'a, L> {
    Whole {
        entries: &'a IndexedValue<RuntimeValue>,
        leaf: Option<&'a L>,
    },
    Presented(&'a IndexedValue<Presented<L>>),
}

impl<L> Clone for EntriesRef<'_, L> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<L> Copy for EntriesRef<'_, L> {}

impl<L> Clone for EntriesNode<'_, L> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<L> Copy for EntriesNode<'_, L> {}

impl<L> Presented<L> {
    /// This whole value, borrowed.
    #[must_use]
    pub const fn as_ref(&self) -> PresentedRef<'_, L> {
        PresentedRef(RefNode::Presented(self))
    }
}

impl<'a, L> PresentedRef<'a, L> {
    /// A value without a presentation, borrowed.
    #[must_use]
    pub const fn plain(value: &'a RuntimeValue) -> Self {
        Self(RefNode::Whole { value, leaf: None })
    }

    /// The outermost level of this part.
    #[must_use]
    pub const fn view(self) -> PresentedView<'a, L> {
        match self.0 {
            RefNode::Whole { value, leaf } => PresentedView::Whole { value, leaf },
            RefNode::Presented(presented) => presented.view(),
        }
    }

    /// The entries of an indexed value; `None` for any other value.
    #[must_use]
    pub const fn entries(self) -> Option<EntriesRef<'a, L>> {
        let (value, leaf) = match self.0 {
            RefNode::Whole { value, leaf } => (value, leaf),
            RefNode::Presented(presented) => match &presented.0 {
                Node::Whole { value, leaf } => (value, leaf.as_ref()),
                Node::Indexed(entries) => {
                    return Some(EntriesRef(EntriesNode::Presented(entries)));
                }
                Node::Struct(_) => return None,
            },
        };
        match value {
            RuntimeValue::Indexed(entries) => {
                Some(EntriesRef(EntriesNode::Whole { entries, leaf }))
            }
            _ => None,
        }
    }

    /// The value of `field` of a struct value; `None` when this is not a
    /// struct value or has no such field.
    #[must_use]
    pub fn field(self, field: &FieldName) -> Option<Self> {
        let (value, leaf) = match self.0 {
            RefNode::Whole { value, leaf } => (value, leaf),
            RefNode::Presented(presented) => match &presented.0 {
                Node::Whole { value, leaf } => (value, leaf.as_ref()),
                Node::Struct(fields) => return fields.field(field).map(Presented::as_ref),
                Node::Indexed(_) => return None,
            },
        };
        match value {
            RuntimeValue::Struct(fields) => fields
                .field(field)
                .map(|value| Self(RefNode::Whole { value, leaf })),
            _ => None,
        }
    }

    /// An owned copy of this part; `clone_value` copies a part of a whole
    /// value.
    #[must_use]
    pub fn to_owned_with(
        self,
        clone_value: impl FnOnce(&RuntimeValue) -> RuntimeValue,
    ) -> Presented<L>
    where
        L: Clone,
    {
        match self.0 {
            RefNode::Whole { value, leaf } => Presented(Node::Whole {
                value: clone_value(value),
                leaf: leaf.cloned(),
            }),
            RefNode::Presented(presented) => presented.clone(),
        }
    }
}

impl<'a, L> EntriesRef<'a, L> {
    /// The index of the entries.
    #[must_use]
    pub fn index(self) -> &'a graphcal_compiler::semantic::checked_type::IndexTypeRef {
        match self.0 {
            EntriesNode::Whole { entries, .. } => entries.index(),
            EntriesNode::Presented(entries) => entries.index(),
        }
    }

    /// The entry for `key`, when `key` belongs to the axis.
    #[must_use]
    pub fn get(self, key: &IndexEntryKey) -> Option<PresentedRef<'a, L>> {
        match self.0 {
            EntriesNode::Whole { entries, leaf } => entries
                .get(key)
                .map(|value| PresentedRef(RefNode::Whole { value, leaf })),
            EntriesNode::Presented(entries) => entries.get(key).map(Presented::as_ref),
        }
    }

    /// The entry `key` selects (see [`IndexedValue::get_key`]).
    #[must_use]
    pub fn get_key(self, key: &KeyValue) -> Option<PresentedRef<'a, L>> {
        match self.0 {
            EntriesNode::Whole { entries, leaf } => entries
                .get_key(key)
                .map(|value| PresentedRef(RefNode::Whole { value, leaf })),
            EntriesNode::Presented(entries) => entries.get_key(key).map(Presented::as_ref),
        }
    }
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::dag_id::DagId;
    use graphcal_compiler::resolved_name::ResolvedStructTypeName;
    use graphcal_compiler::semantic::unit_scale::PositiveFiniteScale;
    use graphcal_compiler::syntax::index_name::IndexEntryKey;
    use graphcal_compiler::syntax::type_name::{ConstructorName, FieldName, StructTypeName};

    use super::{Presented, PresentedRef, PresentedView, ResolvedValue};
    use crate::presentation_evidence::{PresentationFailure, QuantityDisplay, ResolvedLeaf};
    use crate::runtime_value::{IndexAxis, IndexedValue, KeyValue, RuntimeValue, StructValue};

    fn quantity(value: f64) -> RuntimeValue {
        RuntimeValue::quantity(value).unwrap()
    }

    fn unit(label: &str) -> ResolvedLeaf {
        ResolvedLeaf::Quantity(QuantityDisplay::Unit {
            label: label.to_owned(),
            scale: PositiveFiniteScale::new(1000.0).unwrap(),
        })
    }

    fn failed() -> ResolvedLeaf {
        ResolvedLeaf::Quantity(QuantityDisplay::Failed(PresentationFailure::Projection {
            message: "overflow".to_owned(),
        }))
    }

    fn zone() -> ResolvedLeaf {
        ResolvedLeaf::Datetime(
            graphcal_compiler::semantic::time_zone::TimeZoneRegistry::bundled()
                .parse_iana_id("Asia/Tokyo")
                .unwrap(),
        )
    }

    fn datetime() -> RuntimeValue {
        RuntimeValue::Datetime(hifitime::Epoch::from_gregorian_utc_at_midnight(2026, 1, 1))
    }

    fn field(name: &str) -> FieldName {
        FieldName::expect_valid(name)
    }

    fn pair_of<V>(constructor: &str, left: V, right: V) -> StructValue<V> {
        let quantity = || {
            graphcal_compiler::semantic::checked_type::CheckedType::Quantity(
                graphcal_compiler::dimension::Dimension::dimensionless(),
            )
        };
        StructValue::for_test(
            ResolvedStructTypeName::for_test(
                DagId::root_in_package("presented", "main"),
                StructTypeName::expect_valid("Pair"),
            ),
            ConstructorName::expect_valid(constructor),
            vec![
                (field("left"), quantity(), left),
                (field("right"), quantity(), right),
            ],
        )
    }

    fn leaf_of(presented: &ResolvedValue) -> Option<&ResolvedLeaf> {
        match presented.view() {
            PresentedView::Whole { leaf, .. } => leaf,
            PresentedView::Struct(_) | PresentedView::Indexed(_) => {
                panic!("expected a whole value")
            }
        }
    }

    fn whole_label(presented: &ResolvedValue) -> Option<String> {
        match leaf_of(presented) {
            Some(ResolvedLeaf::Quantity(QuantityDisplay::Unit { label, .. })) => {
                Some(label.clone())
            }
            _ => None,
        }
    }

    #[test]
    fn a_leaf_presents_only_values_of_its_kind() {
        let entries = IndexedValue::finite_for_test(vec![quantity(1.0), quantity(2.0)]);
        assert!(Presented::with_leaf(RuntimeValue::Indexed(entries), unit("km")).is_ok());
        assert!(Presented::with_leaf(quantity(1.0), failed()).is_ok());
        assert!(Presented::with_leaf(datetime(), zone()).is_ok());
        assert!(Presented::with_leaf(quantity(1.0), zone()).is_err());
        assert!(Presented::with_leaf(datetime(), unit("km")).is_err());
        assert!(Presented::with_leaf(RuntimeValue::Bool(true), unit("km")).is_err());
        let mixed = IndexedValue::finite_for_test(vec![quantity(1.0), RuntimeValue::Int(2)]);
        assert!(Presented::with_leaf(RuntimeValue::Indexed(mixed), unit("km")).is_err());
        let fields = pair_of("Pair", quantity(1.0), quantity(2.0));
        assert!(Presented::with_leaf(RuntimeValue::Struct(fields), unit("km")).is_err());
    }

    #[test]
    fn containers_keep_presented_children_and_collapse_when_plain() {
        let plain = Presented::<ResolvedLeaf>::from_indexed(IndexedValue::finite_for_test(vec![
            Presented::plain(quantity(1.0)),
            Presented::plain(quantity(2.0)),
        ]));
        assert!(plain.is_plain());
        assert_eq!(plain.retained_nodes(), 0);
        assert!(matches!(
            &*plain.value(),
            RuntimeValue::Indexed(entries) if entries.values().len() == 2
        ));

        let presented = Presented::from_indexed(IndexedValue::finite_for_test(vec![
            Presented::with_leaf(quantity(1.0), unit("km")).unwrap(),
            Presented::plain(quantity(2.0)),
        ]));
        assert!(!presented.is_plain());
        assert_eq!(presented.retained_nodes(), 2);
        assert!(matches!(presented.view(), PresentedView::Indexed(_)));
        let entries = presented.clone().into_entries().unwrap();
        assert_eq!(
            whole_label(entries.get(&IndexEntryKey::position(0)).unwrap()),
            Some("km".to_owned())
        );
        assert!(entries.get(&IndexEntryKey::position(1)).unwrap().is_plain());
        assert!(presented.clone().into_fields().is_none());
        let RuntimeValue::Indexed(value) = presented.into_value() else {
            panic!("indexed value");
        };
        assert_eq!(value.values().len(), 2);

        let fields = Presented::from_struct(pair_of(
            "Pair",
            Presented::with_leaf(quantity(1.0), unit("km")).unwrap(),
            Presented::plain(quantity(2.0)),
        ));
        assert!(matches!(fields.view(), PresentedView::Struct(_)));
        assert!(fields.clone().into_entries().is_none());
        let fields = fields.into_fields().unwrap();
        assert_eq!(
            whole_label(fields.field(&field("left")).unwrap()),
            Some("km".to_owned())
        );
        assert!(
            Presented::<ResolvedLeaf>::from_struct(pair_of(
                "Pair",
                Presented::plain(quantity(1.0)),
                Presented::plain(quantity(2.0)),
            ))
            .is_plain()
        );
    }

    #[test]
    fn a_whole_leaf_reaches_every_part_of_its_value() {
        let entries = IndexedValue::finite_for_test(vec![quantity(1.0), quantity(2.0)]);
        let whole = Presented::with_leaf(RuntimeValue::Indexed(entries), unit("km")).unwrap();
        let parts = whole.clone().into_entries().unwrap();
        assert!(
            parts
                .values()
                .iter()
                .all(|part| whole_label(part) == Some("km".to_owned()))
        );
        let entries = whole.as_ref().entries().unwrap();
        let selected = entries.get(&IndexEntryKey::position(1)).unwrap();
        assert_eq!(
            whole_label(&selected.to_owned_with(RuntimeValue::clone)),
            Some("km".to_owned())
        );
        let key = KeyValue::at(
            IndexAxis::finite(
                graphcal_compiler::semantic::index_def::FiniteIndex::try_from_u64(2).unwrap(),
            )
            .unwrap(),
            0,
        )
        .unwrap();
        assert!(entries.get_key(&key).is_some());
        assert!(entries.get(&IndexEntryKey::position(5)).is_none());
        assert!(selected.entries().is_none());
        assert!(selected.field(&field("left")).is_none());
        assert!(whole.as_ref().field(&field("left")).is_none());

        let fields = Presented::from_struct(pair_of(
            "Pair",
            Presented::with_leaf(quantity(1.0), unit("km")).unwrap(),
            Presented::plain(quantity(2.0)),
        ));
        let left = fields.as_ref().field(&field("left")).unwrap();
        assert_eq!(
            whole_label(&left.to_owned_with(RuntimeValue::clone)),
            Some("km".to_owned())
        );
        assert!(fields.as_ref().entries().is_none());
        let plain = RuntimeValue::Struct(pair_of("Pair", quantity(1.0), quantity(2.0)));
        let right = PresentedRef::<ResolvedLeaf>::plain(&plain)
            .field(&field("right"))
            .unwrap();
        assert!(right.to_owned_with(RuntimeValue::clone).is_plain());
    }

    #[test]
    fn an_unannotated_step_keeps_the_initial_presentation_of_its_constructor() {
        let initial = Presented::from_struct(pair_of(
            "Pair",
            Presented::with_leaf(quantity(1.0), unit("km")).unwrap(),
            Presented::plain(quantity(2.0)),
        ));
        let same = Presented::plain(RuntimeValue::Struct(pair_of(
            "Pair",
            quantity(3.0),
            quantity(4.0),
        )))
        .with_default_presentation(&initial)
        .unwrap();
        let fields = same.into_fields().unwrap();
        assert_eq!(
            whole_label(fields.field(&field("left")).unwrap()),
            Some("km".to_owned())
        );
        assert!(fields.field(&field("right")).unwrap().is_plain());

        // Another constructor of the union: the initial display describes
        // none of its fields.
        let other = Presented::plain(RuntimeValue::Struct(pair_of(
            "Other",
            quantity(3.0),
            quantity(4.0),
        )))
        .with_default_presentation(&initial)
        .unwrap();
        assert!(other.is_plain());

        // A presented step keeps its own presentation.
        let own = Presented::with_leaf(quantity(5.0), unit("mm"))
            .unwrap()
            .with_default_presentation(&Presented::with_leaf(quantity(1.0), unit("km")).unwrap())
            .unwrap();
        assert_eq!(whole_label(&own), Some("mm".to_owned()));

        let indexed = Presented::from_indexed(IndexedValue::finite_for_test(vec![
            Presented::with_leaf(quantity(1.0), unit("km")).unwrap(),
            Presented::plain(quantity(2.0)),
        ]));
        let step = Presented::plain(RuntimeValue::Indexed(IndexedValue::finite_for_test(vec![
            quantity(3.0),
            quantity(4.0),
        ])))
        .with_default_presentation(&indexed)
        .unwrap();
        assert_eq!(step.retained_nodes(), 2);

        // A value of another type is a violated invariant, never plain.
        assert!(
            Presented::plain(quantity(3.0))
                .with_default_presentation(&initial)
                .is_err()
        );
        assert!(
            Presented::plain(RuntimeValue::Bool(true))
                .with_default_presentation(
                    &Presented::with_leaf(quantity(1.0), unit("km")).unwrap()
                )
                .is_err()
        );
        assert!(
            Presented::plain(RuntimeValue::Bool(true))
                .with_default_presentation(&Presented::<ResolvedLeaf>::plain(quantity(1.0)))
                .unwrap()
                .is_plain()
        );
    }

    #[test]
    fn resolution_keeps_every_leaf_kind() {
        use crate::presentation_evidence::{PendingLeaf, PendingQuantityDisplay};
        let pending = Presented::from_struct(pair_of(
            "Pair",
            Presented::with_leaf(
                quantity(1.0),
                PendingLeaf::Quantity(PendingQuantityDisplay::Ready(QuantityDisplay::Unit {
                    label: "km".to_owned(),
                    scale: PositiveFiniteScale::new(1000.0).unwrap(),
                })),
            )
            .unwrap(),
            Presented::plain(quantity(2.0)),
        ));
        let mapped = pending
            .clone()
            .try_map_quantity_displays(|display| {
                Ok::<_, ()>(match display {
                    PendingQuantityDisplay::Ready(_) => PendingQuantityDisplay::Ready(
                        QuantityDisplay::Failed(PresentationFailure::Projection {
                            message: "replaced".to_owned(),
                        }),
                    ),
                    requested @ PendingQuantityDisplay::Requested(_) => requested,
                })
            })
            .unwrap();
        let resolved = mapped
            .try_resolve(|display| match display {
                PendingQuantityDisplay::Ready(display) => Ok::<_, ()>(display),
                PendingQuantityDisplay::Requested(_) => Err(()),
            })
            .unwrap();
        let fields = resolved.into_fields().unwrap();
        assert!(matches!(
            leaf_of(fields.field(&field("left")).unwrap()),
            Some(ResolvedLeaf::Quantity(QuantityDisplay::Failed(_)))
        ));
        let zoned = Presented::with_leaf(
            datetime(),
            PendingLeaf::Datetime(match zone() {
                ResolvedLeaf::Datetime(zone) => zone,
                ResolvedLeaf::Quantity(_) => panic!("a time zone leaf"),
            }),
        )
        .unwrap()
        .try_resolve(|_| Err::<QuantityDisplay, ()>(()))
        .unwrap();
        assert!(matches!(leaf_of(&zoned), Some(ResolvedLeaf::Datetime(_))));
        assert!(
            pending
                .try_resolve(|_| Err::<QuantityDisplay, &str>("failed"))
                .is_err()
        );
    }
}
