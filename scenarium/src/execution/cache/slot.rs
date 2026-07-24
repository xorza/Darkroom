use crate::DynamicValue;
use crate::execution::digest::Digest;
use crate::node::lambda::OutputDemand;
use crate::runtime::any_state::AnyState;
use crate::runtime::shared_any_state::SharedAnyState;

#[derive(Debug)]
pub(crate) struct OutputSnapshot {
    pub(crate) values: Vec<DynamicValue>,
}

impl OutputSnapshot {
    pub(crate) fn new(values: Vec<DynamicValue>) -> Self {
        Self { values }
    }

    fn empty(output_count: usize) -> Self {
        Self::new(vec![DynamicValue::Unbound; output_count])
    }

    fn reset(&mut self, output_count: usize) {
        self.values.clear();
        self.values.resize(output_count, DynamicValue::Unbound);
    }

    pub(crate) fn covers_demand(&self, demand: &[OutputDemand]) -> bool {
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

/// Whether one node's cached output is resident. Disk availability is discovered on demand
/// from the node's digest rather than mirrored in runtime state.
#[derive(Default, Debug)]
pub(crate) enum ValueState {
    /// No cached output — never produced, evicted, or cleared for re-execution.
    #[default]
    Empty,
    /// Values resident in RAM. `produced_under` is the digest they were computed
    /// under — `None` for an impure node, which holds a value but is never a hit.
    Resident {
        snapshot: OutputSnapshot,
        produced_under: Option<Digest>,
    },
}

/// One node's cross-run runtime state: the [`value`](RuntimeSlot::value) cache and
/// the node's persistent `state`/`event_state`.
#[derive(Default, Debug)]
pub(crate) struct RuntimeSlot {
    pub(crate) state: AnyState,
    pub(crate) event_state: SharedAnyState,
    /// The node's current content digest — its cache-validity key (`None` when not
    /// reproducible), stamped producer-first by the resolver and refreshed at execution
    /// reach only for a late bound-resource identity ([`digest::node_digest`]). A resident
    /// value hits iff its
    /// `produced_under` equals this — so a flipped-back input can't serve a stale value.
    pub(crate) current_digest: Option<Digest>,
    pub(crate) value: ValueState,
}

/// The two slot fields the run loop hands a lambda — its persistent `state` and the
/// fresh output buffer — split-borrowed from one slot so both can be written at once.
/// Produced by [`RuntimeSlot::invoke_slot`].
pub(crate) struct InvokeSlot<'a> {
    pub(crate) state: &'a mut AnyState,
    pub(crate) outputs: &'a mut Vec<DynamicValue>,
}

impl RuntimeSlot {
    /// Drop the cached output, leaving the persistent `state`/`event_state` intact.
    /// The run loop calls this on the failure paths — a node that errored or was
    /// skipped for an errored dependency — so a stale prior value isn't left resident
    /// as if it were this run's result. (Successful runs reuse the buffer in place;
    /// see [`invoke_slot`](Self::invoke_slot).)
    pub(crate) fn clear_output(&mut self) {
        self.value = ValueState::Empty;
    }

    /// The resident output values, or `None` when the slot isn't `Resident`.
    pub(crate) fn output_values(&self) -> Option<&Vec<DynamicValue>> {
        match &self.value {
            ValueState::Resident { snapshot, .. } => Some(&snapshot.values),
            _ => None,
        }
    }

    /// Reject stale resident references so they cannot enter a new resource-backed digest.
    pub(crate) fn current_output_values(&self) -> Option<&[DynamicValue]> {
        match &self.value {
            ValueState::Resident {
                snapshot,
                produced_under,
            } if self.current_digest.is_some() && *produced_under == self.current_digest => {
                Some(&snapshot.values)
            }
            _ => None,
        }
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
        let ValueState::Resident { produced_under, .. } = &mut self.value else {
            panic!("a node's output must be resident when it is stamped produced");
        };
        *produced_under = digest;
    }
}
