//! Pure static shape contract for plot encoding channels.
//!
//! Plot expressions are inferred elsewhere. This module receives only
//! plottable leaf kinds and canonical index axes, so a checked channel shape
//! cannot represent a struct, complex value, or another unsupported leaf.

use thiserror::Error;

use crate::dimension::Dimension;
use crate::registry::declared_type::IndexTypeRef;
use crate::registry::time_scale::TimeScale;

/// A leaf value that can be represented by a plot encoding channel.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PlotLeafKind {
    Quantity(Dimension),
    Int,
    Bool,
    Datetime(TimeScale),
    Key(IndexTypeRef),
    /// A contextual string literal accepted directly by plot syntax.
    ContextualString,
}

/// Static type shape of one plot encoding channel.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlotChannelShape {
    /// Axes from the outermost to the innermost indexed collection.
    axes: Vec<IndexTypeRef>,
    leaf: PlotLeafKind,
}

impl PlotChannelShape {
    #[must_use]
    pub const fn new(axes: Vec<IndexTypeRef>, leaf: PlotLeafKind) -> Self {
        Self { axes, leaf }
    }

    #[must_use]
    pub fn axes(&self) -> &[IndexTypeRef] {
        &self.axes
    }

    #[must_use]
    pub const fn leaf(&self) -> &PlotLeafKind {
        &self.leaf
    }
}

/// Validated mapping from each channel's axes to one shared row-axis set.
///
/// `row_channel` is absent only for an empty input. Every mapping has the same
/// source order as the input channels and contains one row-axis position per
/// axis in that channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlotAxisAlignment {
    row_channel: Option<usize>,
    channel_positions: Vec<Vec<usize>>,
}

impl PlotAxisAlignment {
    #[must_use]
    pub const fn row_channel(&self) -> Option<usize> {
        self.row_channel
    }

    #[must_use]
    pub fn channel_positions(&self) -> &[Vec<usize>] {
        &self.channel_positions
    }
}

/// Failure to map one channel onto the widest channel's canonical axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("plot channel {channel} does not range over a subset of row channel {row_channel}")]
pub struct PlotAxisAlignmentError {
    channel: usize,
    row_channel: usize,
}

impl PlotAxisAlignmentError {
    #[must_use]
    pub const fn channel(self) -> usize {
        self.channel
    }

    #[must_use]
    pub const fn row_channel(self) -> usize {
        self.row_channel
    }
}

/// Align canonical channel axes onto the axes of the widest channel.
///
/// Axis matching is owner-aware and multiplicity-aware: repeated uses of one
/// index consume distinct positions in the row shape. Scalar channels have an
/// empty axis slice and therefore broadcast to every row.
///
/// # Errors
///
/// Returns [`PlotAxisAlignmentError`] when any channel is not a subset of the
/// selected row channel's axes.
pub fn align_plot_channel_axes(
    channels: &[&[IndexTypeRef]],
) -> Result<PlotAxisAlignment, PlotAxisAlignmentError> {
    let Some((row_channel, row_axes)) = channels.iter().enumerate().reduce(|widest, candidate| {
        if candidate.1.len() > widest.1.len() {
            candidate
        } else {
            widest
        }
    }) else {
        return Ok(PlotAxisAlignment {
            row_channel: None,
            channel_positions: Vec::new(),
        });
    };

    let channel_positions = channels
        .iter()
        .enumerate()
        .map(|(channel, axes)| {
            map_channel_axes(axes, row_axes).ok_or(PlotAxisAlignmentError {
                channel,
                row_channel,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(PlotAxisAlignment {
        row_channel: Some(row_channel),
        channel_positions,
    })
}

fn map_channel_axes(
    channel_axes: &[IndexTypeRef],
    row_axes: &[IndexTypeRef],
) -> Option<Vec<usize>> {
    let mut used = vec![false; row_axes.len()];
    channel_axes
        .iter()
        .map(|axis| {
            let position = row_axes
                .iter()
                .enumerate()
                .position(|(index, row_axis)| !used[index] && row_axis.matches_ref(axis))?;
            used[position] = true;
            Some(position)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_id::DagId;
    use crate::syntax::index_name::{IndexName, ResolvedIndexName};

    fn axis(owner: &str, name: &str) -> IndexTypeRef {
        IndexTypeRef::from_resolved(ResolvedIndexName::from_def(
            DagId::root_in_package("test", owner),
            IndexName::expect_valid(name),
        ))
    }

    #[test]
    fn aligns_subsets_and_scalar_broadcasts() {
        let phase = axis("main", "Phase");
        let time = axis("main", "Time");
        let scalar = Vec::new();
        let phase_only = vec![phase.clone()];
        let phase_time = vec![phase, time];
        let channels = [
            scalar.as_slice(),
            phase_only.as_slice(),
            phase_time.as_slice(),
        ];

        let alignment = align_plot_channel_axes(&channels).unwrap();

        assert_eq!(alignment.row_channel(), Some(2));
        assert_eq!(
            alignment.channel_positions(),
            &[vec![], vec![0], vec![0, 1]]
        );
    }

    #[test]
    fn repeated_axes_consume_distinct_row_positions() {
        let phase = axis("main", "Phase");
        let once = vec![phase.clone()];
        let twice = vec![phase.clone(), phase];
        let channels = [once.as_slice(), twice.as_slice()];

        let alignment = align_plot_channel_axes(&channels).unwrap();

        assert_eq!(alignment.channel_positions(), &[vec![0], vec![0, 1]]);
    }

    #[test]
    fn same_leaf_axes_from_distinct_owners_are_incompatible() {
        let first = vec![axis("first", "Phase")];
        let second = vec![axis("second", "Phase")];
        let channels = [first.as_slice(), second.as_slice()];

        assert_eq!(
            align_plot_channel_axes(&channels),
            Err(PlotAxisAlignmentError {
                channel: 1,
                row_channel: 0,
            })
        );
    }
}
