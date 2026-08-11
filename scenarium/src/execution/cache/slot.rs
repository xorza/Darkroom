use crate::DynamicValue;
use crate::execution::cache::digest::Digest;
use crate::execution::cache::disk_store::error::StoreResult;
use crate::execution::cache::disk_store::store_outcome::StoreOutcome;
use crate::graph::func::lambda::OutputDemand;
use crate::graph::identity::FuncId;
use crate::runtime::any_state::AnyState;
use crate::runtime::shared_any_state::SharedAnyState;

#[derive(Debug)]
pub(crate) struct OutputSnapshot {
    values: Vec<DynamicValue>,
}

impl OutputSnapshot {
    pub(crate) fn new(values: Vec<DynamicValue>) -> Self {
        Self { values }
    }

    /// The values, in output-port order. Read-only outside this file: a
    /// snapshot's length is its node's arity, so nothing but the slot that
    /// owns it may resize or replace what it holds.
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

/// Whether one node's cached output is resident, and whether disk backs it.
///
/// Private to the slot, like the field holding it: every read and write goes
/// through a method below, so "resident" has one definition and no caller can
/// leave the slot in a shape the slot itself would not produce.
#[derive(Default, Debug)]
enum ValueState {
    /// No cached output — never produced, evicted, or cleared for re-execution.
    #[default]
    Empty,
    /// Values resident in RAM. `produced_under` is the digest they were computed
    /// under — `None` for an impure node, which holds a value but is never a hit.
    Resident {
        snapshot: OutputSnapshot,
        produced_under: Option<Digest>,
        /// Whether a blob under `produced_under` is believed to be on disk.
        ///
        /// Rides *with* the value because that is the only time it is ever read:
        /// a reuse consults it exactly when serving from RAM, and a value that
        /// is gone has no durability left to be wrong about. Dropping the value
        /// therefore drops the belief, with no second call to forget.
        on_disk: bool,
    },
}

/// One node's cross-run runtime state: the [`value`](RuntimeSlot::value) cache and
/// the node's persistent `state`/`event_state`, owned by the implementation
/// recorded in [`owner`](RuntimeSlot::owner).
#[derive(Default, Debug)]
pub(crate) struct RuntimeSlot {
    /// The func whose implementation owns this slot's `state`/`event_state`.
    /// Output values are digest-keyed (the digest folds the func id), but state
    /// survives installs untouched — so [`RuntimeSlot::reown`] drops it when the
    /// installed node's implementation no longer matches.
    pub(crate) owner: FuncId,
    pub(crate) state: AnyState,
    pub(crate) event_state: SharedAnyState,
    /// The node's current content digest — its cache-validity key (`None` when not
    /// reproducible), stamped producer-first by the resolver and refreshed at execution
    /// reach only for a late bound-path identity
    /// ([`RuntimeCache::node_digest`](crate::execution::cache::runtime::RuntimeCache::node_digest)).
    /// A resident
    /// value hits iff its
    /// `produced_under` equals this — so a flipped-back input can't serve a stale value.
    pub(crate) current_digest: Option<Digest>,
    value: ValueState,
}

/// The two slot fields the run loop hands a lambda — its persistent `state` and the
/// fresh output buffer — split-borrowed from one slot so both can be written at once.
/// Produced by [`RuntimeSlot::invoke_slot`].
#[derive(Debug)]
pub(crate) struct InvokeSlot<'a> {
    pub(crate) state: &'a mut AnyState,
    pub(crate) outputs: &'a mut Vec<DynamicValue>,
}

impl RuntimeSlot {
    /// The slot a node gains when an install first brings it in: no value, no
    /// digest, owned by the implementation compiled for it.
    pub(super) fn new(owner: FuncId) -> Self {
        Self {
            owner,
            ..Default::default()
        }
    }

    /// Re-stamp the slot for the installed node's implementation, dropping `state`
    /// and `event_state` when a different one owned them — a changed function must
    /// not inherit state written by its predecessor. The output value stays: its
    /// validity is digest-keyed and the digest already folds the owner.
    pub(super) fn reown(&mut self, owner: FuncId) {
        if self.owner == owner {
            return;
        }
        self.owner = owner;
        self.state.clear();
        self.event_state = SharedAnyState::default();
    }

    /// Drop the cached output, leaving the persistent `state`/`event_state` intact.
    /// The run loop calls this on the failure paths — a node that errored or was
    /// skipped for an errored dependency — so a stale prior value isn't left resident
    /// as if it were this run's result. (Successful runs reuse the buffer in place;
    /// see [`invoke_slot`](Self::invoke_slot).)
    pub(crate) fn clear_output(&mut self) {
        self.value = ValueState::Empty;
    }

    /// Take a whole snapshot decoded from this node's blob — the counterpart to
    /// [`clear_output`](Self::clear_output), and the only way a value arrives
    /// whole rather than port by port through
    /// [`invoke_slot`](Self::invoke_slot).
    ///
    /// Reading a blob is itself proof it is there, so residency and durability
    /// are established in one move rather than stamped separately by each
    /// caller.
    pub(crate) fn load_from_blob(&mut self, snapshot: OutputSnapshot, digest: Digest) {
        self.value = ValueState::Resident {
            snapshot,
            produced_under: Some(digest),
            on_disk: true,
        };
    }

    /// Whether a blob backing the resident value is believed to be on disk.
    ///
    /// Belief, not proof: it records what this engine last wrote or read, so a
    /// blob deleted behind its back still reads as present. Proving it would
    /// take a read per node per run, which is what this exists to avoid — a
    /// user who suspects the store is the one who reaches for a flush.
    ///
    /// No digest to pass: this is only ever asked of a value already established
    /// as a hit, and [`current_snapshot`](Self::current_snapshot) settled which
    /// digest that was.
    pub(crate) fn blob_is_current(&self) -> bool {
        matches!(self.value, ValueState::Resident { on_disk: true, .. })
    }

    /// Record what a store left on disk for the resident value.
    ///
    /// A write that did not land leaves the node owing one, and the next
    /// resident reuse tries again — which is what makes a transient full disk
    /// heal itself rather than persist as a node that lies about being cached.
    pub(crate) fn note_store(&mut self, outcome: &StoreResult) {
        let ValueState::Resident { on_disk, .. } = &mut self.value else {
            return;
        };
        *on_disk = matches!(
            outcome,
            Ok(StoreOutcome::Published | StoreOutcome::AlreadyCovered)
        );
    }

    /// The resident output values, or `None` when the slot holds none.
    pub(crate) fn output_values(&self) -> Option<&[DynamicValue]> {
        match &self.value {
            ValueState::Resident { snapshot, .. } => Some(&snapshot.values),
            _ => None,
        }
    }

    /// Read one resident output port: a clone of the value, or — with `take` —
    /// the value itself, moved out and leaving `Unbound` behind. `None` when the
    /// slot holds no values at all; a port outside the resident ones is a caller
    /// bug and panics.
    ///
    /// The move is the executor's last-read fast path for a non-RAM producer:
    /// the RAM copy would be released right after anyway, and handing over the
    /// slot's copy leaves the consumer the sole `Arc` holder, so
    /// [`DynamicValue::into_custom`] can reuse the allocation in place.
    pub(super) fn read_output(&mut self, port_idx: u32, take: bool) -> Option<DynamicValue> {
        let ValueState::Resident { snapshot, .. } = &mut self.value else {
            return None;
        };
        let value = &mut snapshot.values[port_idx as usize];
        Some(if take {
            std::mem::take(value)
        } else {
            value.clone()
        })
    }

    /// Release a single output value (to `Unbound`), keeping its siblings — the
    /// mid-run per-output release for a non-RAM producer whose one output just
    /// went spent while others are still owed to other consumers.
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

    /// The resident values, but only where they were produced under the digest
    /// the slot currently carries — the one definition of "the bytes are here",
    /// which cache reuse, the executor's input read, and the disk store all rely
    /// on. A `None` current digest (an impure cone) never qualifies, nor does a
    /// value produced under a *different* digest, which is what a changed input
    /// leaves behind.
    ///
    /// It lives here because both halves of the test are this slot's own fields;
    /// a second copy on the cache was a second definition of "current".
    pub(crate) fn current_snapshot(&self) -> Option<&OutputSnapshot> {
        let ValueState::Resident {
            snapshot,
            produced_under,
            ..
        } = &self.value
        else {
            return None;
        };
        (self.current_digest.is_some() && *produced_under == self.current_digest)
            .then_some(snapshot)
    }

    /// Prepare the slot for a lambda invocation and hand back *disjoint* mutable
    /// borrows of `state` and the output buffer — the lambda writes both at once,
    /// which a single whole-slot borrow couldn't allow. A resident buffer is reused
    /// **in place**, cleared to `Unbound`, and `resize`d to the current arity. Clearing
    /// prevents a skipped output from retaining a value produced by an earlier run.
    /// `produced_under` stays as-is until [`stamp_produced`](Self::stamp_produced)
    /// updates it on success.
    pub(crate) fn invoke_slot(&mut self, output_count: usize) -> InvokeSlot<'_> {
        match &mut self.value {
            ValueState::Resident { snapshot, .. } => snapshot.reset(output_count),
            _ => {
                self.value = ValueState::Resident {
                    snapshot: OutputSnapshot::empty(output_count),
                    produced_under: None,
                    on_disk: false,
                };
            }
        }
        let ValueState::Resident { snapshot, .. } = &mut self.value else {
            unreachable!("set to Resident just above");
        };
        InvokeSlot {
            state: &mut self.state,
            outputs: &mut snapshot.values,
        }
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

    /// Stamp the resident value with the node's current content digest on a successful
    /// run: `produced_under` turns it into a cache hit for the next run (RAM) and the
    /// key its disk blob is stored under.
    pub(crate) fn stamp_produced(&mut self) {
        let digest = self.current_digest;
        let ValueState::Resident {
            produced_under,
            on_disk,
            ..
        } = &mut self.value
        else {
            panic!("a node's output must be resident when it is stamped produced");
        };
        *produced_under = digest;
        // A value this run just computed is not on disk until the store that
        // follows says so — and the buffer it was written into may be carrying
        // the previous value's answer.
        *on_disk = false;
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::execution::cache::digest::Digest;
    use crate::execution::cache::slot::{OutputSnapshot, RuntimeSlot, ValueState};

    impl RuntimeSlot {
        /// Seed a value the run loop did not compute and no blob backs — how a
        /// fixture stands a prior run up without one. Production has only
        /// [`load_from_blob`](RuntimeSlot::load_from_blob), which is the one way
        /// a whole value legitimately arrives from outside the run loop.
        pub(crate) fn load_output(
            &mut self,
            snapshot: OutputSnapshot,
            produced_under: Option<Digest>,
        ) {
            self.value = ValueState::Resident {
                snapshot,
                produced_under,
                on_disk: false,
            };
        }
    }
}
