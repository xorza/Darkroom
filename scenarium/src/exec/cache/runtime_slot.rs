use crate::data::data_type::DataType;
use crate::data::dynamic_value::DynamicValue;
use crate::decl::func::func_lambda::OutputDemand;
use crate::exec::cache::digest::Digest;
use crate::exec::cache::disk_store::error::StoreResult;
use crate::exec::cache::disk_store::store_outcome::StoreOutcome;
use std::mem;

#[derive(Debug)]
pub(crate) struct OutputSnapshot {
    values: Vec<DynamicValue>,
}

impl OutputSnapshot {
    pub(crate) const fn new(values: Vec<DynamicValue>) -> Self {
        Self { values }
    }

    /// The values, in output-port order. Read-only outside this file: a snapshot's length is its
    /// node's arity, so nothing but the owning slot may resize or replace what it holds.
    pub(crate) fn values(&self) -> &[DynamicValue] {
        &self.values
    }

    fn empty(output_count: usize) -> Self {
        Self::new(vec![DynamicValue::Unbound; output_count])
    }

    fn reset(&mut self, output_count: usize) {
        self.values.clear();
        self.values.resize(output_count, DynamicValue::Unbound);
    }

    pub(super) fn covers_demand(&self, demand: &[OutputDemand]) -> bool {
        debug_assert_eq!(
            self.values.len(),
            demand.len(),
            "cached output values must match output demand arity"
        );
        self.values
            .iter()
            .zip(demand)
            .all(|(value, demand)| !matches!(value, DynamicValue::Unbound) || demand.is_skip())
    }
}

/// Whether one node's cached output is resident.
///
/// Private to the slot, so "resident" has one definition and no caller can leave the slot in a
/// shape the slot itself would not produce.
#[derive(Default, Debug)]
enum ValueState {
    /// No cached output — never produced, evicted, or cleared for re-execution.
    #[default]
    Empty,
    /// Values resident in RAM. `produced_under` is the digest they were computed
    /// under — `None` for an unkeyed node, which holds a value but is never a hit.
    Resident {
        snapshot: OutputSnapshot,
        produced_under: Option<Digest>,
    },
}

/// A node's cache key this run, stamped producer-first by the resolver
/// ([`RuntimeCache::node_digest`](crate::exec::cache::runtime_cache::RuntimeCache::node_digest)).
///
/// Three cases where an `Option<Digest>` held two: an impure cone, which no later stamp keys, and
/// a key that waits on a path, which the run loop stamps again at the node's turn. Held apart, a
/// node of an impure cone is not stamped twice for nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Stamp {
    /// Not reproducible: impure, or reading a producer that is. Final for the run.
    #[default]
    Unkeyed,
    /// Reproducible, but a path it reads was not identified at the sweep, or a producer's key
    /// waits on one: the run loop stamps it again at its turn.
    Deferred,
    Keyed(Digest),
}

impl Stamp {
    /// The key, where the node has one.
    pub(crate) const fn digest(self) -> Option<Digest> {
        match self {
            Self::Keyed(digest) => Some(digest),
            Self::Unkeyed | Self::Deferred => None,
        }
    }
}

/// What disk holds for the node's resident value.
///
/// On the slot rather than the value, so a write that failed stays failed across the values the
/// node makes: the run does not retry it until an install or a new store, or a flush asks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum BlobState {
    /// No blob is believed to back the value; a disk-backed node owes one.
    #[default]
    Owed,
    /// A blob under the value's digest is believed to be on disk.
    Written,
    /// The value holds a type the program's codecs cannot encode, so no blob can be written.
    Unwritable,
    /// The last write failed.
    Failed,
}

/// One node's cross-run output cache: the [`value`](RuntimeSlot::value), the stamp it is checked
/// against, and what disk holds for it.
#[derive(Default, Debug)]
pub(crate) struct RuntimeSlot {
    /// A resident value hits iff its `produced_under` is this stamp's digest.
    stamp: Stamp,
    value: ValueState,
    blob: BlobState,
    /// The last run that ran or served this node, counted by the engine: what memory pressure
    /// drops the least recently used by.
    used_in: u64,
}

/// How [`RuntimeSlot::read_output`] hands a resident value over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputRead {
    /// A clone, leaving the slot's copy resident.
    Clone,
    /// The value itself, moved out and leaving `Unbound` behind — the executor's last-read fast
    /// path for a non-RAM producer: the consumer becomes the sole `Arc` holder, so
    /// [`DynamicValue::into_custom`] can reuse the allocation in place.
    Take,
}

impl RuntimeSlot {
    pub(crate) const fn stamp(&self) -> Stamp {
        self.stamp
    }

    pub(crate) const fn set_stamp(&mut self, stamp: Stamp) {
        self.stamp = stamp;
    }

    pub(crate) const fn used_in(&self) -> u64 {
        self.used_in
    }

    /// Record that run `run` ran or served the node.
    pub(crate) const fn mark_used(&mut self, run: u64) {
        self.used_in = run;
    }

    /// Drop the cached output. The run loop calls this on the failure paths — a node that
    /// errored or was skipped for an errored dependency — so a stale prior value isn't left
    /// resident as this run's result. (Successful runs reuse the buffer in place; see
    /// [`invoke_outputs`](Self::invoke_outputs).)
    pub(crate) fn clear_output(&mut self) {
        self.value = ValueState::Empty;
    }

    /// Take a whole snapshot decoded from this node's blob — the counterpart to
    /// [`clear_output`](Self::clear_output), and the only way a value arrives whole rather than
    /// port by port through [`invoke_outputs`](Self::invoke_outputs). Reading a blob is itself
    /// proof it is there, so residency and durability are established in one move.
    pub(crate) fn load_from_blob(&mut self, snapshot: OutputSnapshot, digest: Digest) {
        self.value = ValueState::Resident {
            snapshot,
            produced_under: Some(digest),
        };
        self.blob = BlobState::Written;
    }

    /// Whether the resident value still owes a blob: none is believed to be on disk, none was
    /// found impossible to write, and no write failed.
    ///
    /// Belief, not proof: it records what this engine last wrote or read, so a blob deleted behind
    /// its back still reads as present. Proving it would take a read per node per run; a user who
    /// suspects the store reaches for a flush. No digest to pass: this is only asked of a value
    /// already established as a hit by [`cur_snapshot`](Self::cur_snapshot).
    pub(crate) const fn owes_blob(&self) -> bool {
        matches!(self.value, ValueState::Resident { .. }) && matches!(self.blob, BlobState::Owed)
    }

    /// Whether the last write failed, so the run writes nothing until it is reconsidered.
    pub(crate) const fn write_failed(&self) -> bool {
        matches!(self.blob, BlobState::Failed)
    }

    /// Record what a store left on disk for the resident value.
    ///
    /// A write that did not land is not retried by a run: a full disk or a folder it may not
    /// write would fail it on every one, unseen. A value no codec encodes is not retried either:
    /// nothing about it changes until the value or the codecs do.
    pub(crate) const fn note_store(&mut self, outcome: &StoreResult) {
        self.blob = match outcome {
            Ok(StoreOutcome::Published | StoreOutcome::AlreadyCovered) => BlobState::Written,
            Ok(StoreOutcome::Unsupported { .. }) => BlobState::Unwritable,
            Ok(StoreOutcome::NothingOwed) => return,
            Err(_) => BlobState::Failed,
        };
    }

    /// Owe a blob again for a value found unwritable or whose write failed: for an install, which
    /// may bring the codec that was missing, a new store, and a flush someone asked for.
    pub(crate) const fn reconsider_blob(&mut self) {
        if matches!(self.blob, BlobState::Unwritable | BlobState::Failed) {
            self.blob = BlobState::Owed;
        }
    }

    /// The resident output values, or `None` when the slot holds none.
    pub(crate) fn output_values(&self) -> Option<&[DynamicValue]> {
        match &self.value {
            ValueState::Resident { snapshot, .. } => Some(&snapshot.values),
            ValueState::Empty => None,
        }
    }

    /// Read one resident output port the way `read` says. `None` when the slot
    /// holds no values at all; a port outside the resident ones is a caller bug
    /// and panics.
    pub(super) fn read_output(&mut self, port_idx: u32, read: OutputRead) -> Option<DynamicValue> {
        let ValueState::Resident { snapshot, .. } = &mut self.value else {
            return None;
        };
        let value = &mut snapshot.values[port_idx as usize];
        Some(match read {
            OutputRead::Clone => value.clone(),
            OutputRead::Take => mem::take(value),
        })
    }

    /// Release a single output value (to `Unbound`), keeping its siblings — the mid-run release
    /// for a non-RAM producer whose one output went spent while others are still owed.
    pub(crate) fn clear_output_port(&mut self, port_idx: u32) {
        let ValueState::Resident { snapshot, .. } = &mut self.value else {
            panic!("an output can only be released from a resident slot");
        };
        debug_assert!(
            (port_idx as usize) < snapshot.values.len(),
            "output port must be in range"
        );
        snapshot.values[port_idx as usize] = DynamicValue::Unbound;
    }

    /// The resident values, but only where they were produced under the key the slot is stamped
    /// with — the one definition of "the bytes are here", relied on by cache reuse, the
    /// executor's input read, and the disk store. A node with no key never qualifies, nor does a
    /// value produced under a *different* digest.
    pub(crate) fn cur_snapshot(&self) -> Option<&OutputSnapshot> {
        let ValueState::Resident {
            snapshot,
            produced_under,
        } = &self.value
        else {
            return None;
        };
        let digest = self.stamp.digest()?;
        (*produced_under == Some(digest)).then_some(snapshot)
    }

    /// Prepare the slot for a lambda invocation and hand back the output buffer the lambda
    /// writes, one entry per entry of `demand`. A resident buffer is reused **in place**: every
    /// demanded output is cleared to `Unbound`, and so is every other one unless the value was
    /// produced under the key the node is stamped with, where an output the run skips is still
    /// that key's and stays. So a node read on one port, then another, under one key holds both.
    /// `produced_under` stays as-is until [`stamp_produced`](Self::stamp_produced) updates it on
    /// success.
    pub(crate) fn invoke_outputs(&mut self, demand: &[OutputDemand]) -> &mut [DynamicValue] {
        let keeps_skipped = self.cur_snapshot().is_some();
        match &mut self.value {
            ValueState::Resident { snapshot, .. } if keeps_skipped => {
                debug_assert_eq!(
                    snapshot.values.len(),
                    demand.len(),
                    "a current value has the node's arity"
                );
                for (value, demand) in snapshot.values.iter_mut().zip(demand) {
                    if !demand.is_skip() {
                        *value = DynamicValue::Unbound;
                    }
                }
            }
            ValueState::Resident { snapshot, .. } => snapshot.reset(demand.len()),
            ValueState::Empty => {
                self.value = ValueState::Resident {
                    snapshot: OutputSnapshot::empty(demand.len()),
                    produced_under: None,
                };
            }
        }
        let ValueState::Resident { snapshot, .. } = &mut self.value else {
            unreachable!("set to Resident just above");
        };
        &mut snapshot.values
    }

    /// Each output, by position, whose value `types`, the outputs' declared types, does not hold.
    pub(crate) fn mistyped_outputs(&self, types: &[DataType]) -> Vec<usize> {
        let ValueState::Resident { snapshot, .. } = &self.value else {
            panic!("a node's output must be resident immediately after invocation");
        };
        debug_assert_eq!(snapshot.values.len(), types.len());
        types
            .iter()
            .zip(&snapshot.values)
            .enumerate()
            .filter_map(|(output, (data_type, value))| (!value.fits(*data_type)).then_some(output))
            .collect()
    }

    pub(crate) fn unbound_demanded_outputs(&self, demand: &[OutputDemand]) -> Vec<usize> {
        let ValueState::Resident { snapshot, .. } = &self.value else {
            panic!("a node's output must be resident immediately after invocation");
        };
        debug_assert_eq!(
            snapshot.values.len(),
            demand.len(),
            "node output values must match output demand arity"
        );
        demand
            .iter()
            .zip(&snapshot.values)
            .enumerate()
            .filter_map(|(output, (demand, value))| {
                (!demand.is_skip() && matches!(value, DynamicValue::Unbound)).then_some(output)
            })
            .collect()
    }

    /// Stamp the resident value with the node's key on a successful run: `produced_under` makes
    /// it a RAM hit next run and the key its disk blob is stored under.
    pub(crate) fn stamp_produced(&mut self) {
        let digest = self.stamp.digest();
        let ValueState::Resident { produced_under, .. } = &mut self.value else {
            panic!("a node's output must be resident when it is stamped produced");
        };
        *produced_under = digest;
        // Not on disk until the store that follows says so; a failed write stays failed.
        if self.blob != BlobState::Failed {
            self.blob = BlobState::Owed;
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::exec::cache::digest::Digest;
    use crate::exec::cache::runtime_slot::{BlobState, OutputSnapshot, RuntimeSlot, ValueState};

    impl RuntimeSlot {
        /// Seed a value the run loop did not compute and no blob backs — how a fixture stands a
        /// prior run up. Production has only [`load_from_blob`](RuntimeSlot::load_from_blob).
        pub(crate) fn load_output(
            &mut self,
            snapshot: OutputSnapshot,
            produced_under: Option<Digest>,
        ) {
            self.value = ValueState::Resident {
                snapshot,
                produced_under,
            };
            self.blob = BlobState::Owed;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::PathBuf;

    use crate::data::data_type::custom_type::CustomTypeId;
    use crate::exec::cache::digest::Digest;
    use crate::exec::cache::disk_store::error::{StoreError, StoreResult};
    use crate::exec::cache::disk_store::store_outcome::StoreOutcome;
    use crate::exec::cache::runtime_slot::{OutputSnapshot, RuntimeSlot};

    /// Each store outcome settles the debt its own way: a blob on disk pays it; a failed write
    /// and a value no codec encodes are not retried, the failure not even by a new value, until
    /// an install, a new store or a flush reconsiders them.
    #[test]
    fn a_store_outcome_settles_the_blob_debt() {
        let mut slot = RuntimeSlot::default();
        slot.load_output(OutputSnapshot::new(Vec::new()), Some(Digest([1; 32])));
        assert!(slot.owes_blob(), "a value nothing wrote owes its blob");

        let failed = || -> StoreResult {
            Err(StoreError::Dir {
                path: PathBuf::new(),
                source: io::Error::other("disk full"),
            })
        };
        let unsupported = || -> StoreResult {
            Ok(StoreOutcome::Unsupported {
                type_id: CustomTypeId::unique(),
            })
        };
        let rows: [(StoreResult, bool, bool); 5] = [
            (Ok(StoreOutcome::Published), false, false),
            (Ok(StoreOutcome::NothingOwed), false, false),
            (failed(), false, true),
            (Ok(StoreOutcome::AlreadyCovered), false, false),
            (unsupported(), false, false),
        ];
        for (outcome, owes, failed) in rows {
            slot.note_store(&outcome);
            assert_eq!(slot.owes_blob(), owes, "{outcome:?}");
            assert_eq!(slot.write_failed(), failed, "{outcome:?}");
        }

        slot.reconsider_blob();
        assert!(
            slot.owes_blob(),
            "an install reconsiders an unwritable value"
        );
        slot.note_store(&Ok(StoreOutcome::Published));
        slot.reconsider_blob();
        assert!(!slot.owes_blob(), "a written blob is not reconsidered");

        slot.note_store(&failed());
        slot.stamp_produced();
        assert!(
            slot.write_failed(),
            "a new value does not retry a failed write"
        );
        slot.reconsider_blob();
        assert!(slot.owes_blob(), "an install, a new store or a flush does");

        slot.clear_output();
        slot.note_store(&unsupported());
        slot.reconsider_blob();
        assert!(!slot.owes_blob(), "an empty slot owes nothing");
    }
}
