//! [`Stage`]: one stage of a run, in what it holds, and how wide a budget lets it run.

use crate::memory::budget::Budget;

/// The fewest rows a combine reads at once, so a tight budget still makes progress.
pub(crate) const MIN_CHUNK_ROWS: usize = 64;

/// One stage of a run, in what it holds: what it keeps of each frame once done with it, beyond
/// what the caller held; what it holds beside the frames for its whole length; and what one unit
/// of its work adds while in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stage {
    pub(crate) kept: usize,
    pub(crate) held: usize,
    pub(crate) in_flight: usize,
    pub(crate) units: Units,
}

/// What a stage's work comes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Units {
    /// Frames, as many at once as the budget and the workers allow. A frame in work on the spill
    /// tier holds itself too, which no kept set holds for it.
    Frames,
    /// Frames, one at a time.
    Serial,
    /// No work of its own beside what it holds: a pass over the frames as they lie.
    Whole,
    /// The output's `rows`, [`MIN_CHUNK_ROWS`] at once at least unless there are fewer. On the
    /// resident tier the frames are read where they lie, all rows at once; on the spill tier a row
    /// holds `in_flight` of the frames' planes.
    Rows(usize),
}

/// Where a run keeps its frames while a stage works over them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Placement {
    /// In memory, `kept_set` bytes of them beside the stage.
    Resident { kept_set: u64 },
    /// On disk, only the frames in work in memory.
    Spilled,
}

/// How wide a stage runs: its units at once, and the bytes its narrowest step holds past the
/// budget anyway, so the run makes progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Width {
    pub(crate) units: usize,
    pub(crate) overcommit_bytes: u64,
}

impl Stage {
    /// The width under `budget` of this stage over `frames` frames, kept as `placement` says.
    pub(crate) fn width(&self, budget: Budget, frames: usize, placement: Placement) -> Width {
        let (kept_set, spilled) = match placement {
            Placement::Resident { kept_set } => (kept_set, false),
            Placement::Spilled => (0, true),
        };
        let per_unit = match (self.units, spilled) {
            (Units::Whole, _) | (Units::Rows(_), false) => 0,
            (Units::Frames | Units::Serial, true) => self.in_flight.saturating_add(self.kept),
            (Units::Frames | Units::Serial | Units::Rows(_), _) => self.in_flight,
        } as u64;
        let (floor, most) = match self.units {
            Units::Frames => (1, budget.workers().min(frames).max(1)),
            Units::Serial | Units::Whole => (1, 1),
            Units::Rows(rows) => (MIN_CHUNK_ROWS.min(rows), rows),
        };
        let beside = kept_set.saturating_add(self.held as u64);
        let free = budget.usable().saturating_sub(beside);
        let units = match free.checked_div(per_unit) {
            Some(fit) => usize::try_from(fit)
                .unwrap_or(usize::MAX)
                .clamp(floor, most),
            None => most,
        };
        Width {
            units,
            overcommit_bytes: beside
                .saturating_add(per_unit.saturating_mul(floor as u64))
                .saturating_sub(budget.usable()),
        }
    }
}
