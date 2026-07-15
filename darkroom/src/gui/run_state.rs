//! Last graph run's centralized runtime state: per-node execution outcomes
//! and logs, plus the latest worker-pushed values for pinned outputs. One
//! [`RunState`] per [`Editor`], updated as worker reports arrive.
//!
//! Execution dissolves subgraphs and remaps interior node ids, so a run's
//! raw `ExecutionStats` are keyed by *flattened* ids. [`RunState::set_results`]
//! projects them through the compile-phase `FlattenMap` (kept by the engine
//! when it sent the run — the worker doesn't echo it back) to fold each
//! outcome onto the authoring nodes: onto the node itself (unique per editor
//! node — a def's interior aggregates across its instances) and onto every
//! ancestor composite instance, so an instance node reflects its whole
//! subtree. Logs attribute the same way.
//!
//! [`Editor`]: crate::gui::app::editor::Editor

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use lens::Image as LensImage;
#[cfg(test)]
use scenarium::NodeAddress;
use scenarium::NodeId;
use scenarium::OutputAddress;
use scenarium::OutputPort;
use scenarium::RunError;
use scenarium::{DynamicValue, RamUsage};
use scenarium::{ExecutionStats, FlattenMap, LogEntry, PinnedOutputs, RunPhase, RunProgress};

use crate::gui::image_viewer::{RenderedImage, convert_image_value};

/// Longest side of the RGBA8 raster prepared for a pinned-output card.
const PIN_PREVIEW_TEXTURE_DIM: u32 = 256;

/// Per-node execution outcome of the last run. Ordered low→high so a
/// higher-severity status wins when several fold onto one node
/// (`Errored` > `MissingInputs` > `Executed` > `Cached`). `Executed`
/// carries the node's wall-clock run time (seconds). `Running` is the
/// transient live state while a node computes (set directly from
/// `RunProgress`, never produced by the final `set_results`); it carries the
/// instant the node started so the UI can show live elapsed-so-far.
#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub(crate) enum ExecStatus {
    #[default]
    None,
    Cached,
    Executed(f64),
    Running(Instant),
    MissingInputs,
    Errored,
}

impl ExecStatus {
    /// Severity rank, for folding several outcomes onto one editor node
    /// (a subgraph def's interior node runs once per instance; an
    /// instance node aggregates its whole subtree). Higher wins. `Running`
    /// is live-only — it's set directly, never folded through `merged`.
    fn severity(self) -> u8 {
        match self {
            ExecStatus::None => 0,
            ExecStatus::Cached => 1,
            ExecStatus::Executed(_) => 2,
            ExecStatus::Running(_) => 3,
            ExecStatus::MissingInputs => 4,
            ExecStatus::Errored => 5,
        }
    }

    /// Fold two outcomes for the same editor node: two `Executed` times
    /// sum (total compute across instances / subtree); otherwise the
    /// worse status wins.
    fn merged(self, other: ExecStatus) -> ExecStatus {
        match (self, other) {
            (ExecStatus::Executed(a), ExecStatus::Executed(b)) => ExecStatus::Executed(a + b),
            _ if other.severity() >= self.severity() => other,
            _ => self,
        }
    }
}

/// Everything the editor knows about one node from the last run.
#[derive(Default, Debug)]
pub(crate) struct NodeRunState {
    pub(crate) status: ExecStatus,
    pub(crate) logs: Vec<LogEntry>,
    /// Human-readable messages for this run's failures, folded on the same
    /// attribution as `status` (a subgraph instance collects its subtree's).
    /// Empty unless the node errored; drives the inspector's error detail so
    /// a failed node reads e.g. "no light frames provided", not just "errored".
    pub(crate) errors: Vec<String>,
    /// RAM this node's cached output holds after the last run (system vs GPU),
    /// summed across its flattened contributors — a subgraph instance aggregates
    /// its interior. Zero unless the node retains a value; drives the node body's
    /// memory readout.
    pub(crate) ram: RamUsage,
}

/// One pushed pinned output plus its eagerly prepared image thumbnail.
/// The full value remains available for text and memory metadata; image
/// conversion happens on receipt, before the canvas record pass.
#[derive(Debug)]
pub(crate) struct PinnedOutputValue {
    /// Monotonically increasing identity of the worker push that supplied
    /// this value. Views use it to refresh derived textures without retaining
    /// a clone of the source value.
    pub(crate) revision: u64,
    pub(crate) value: DynamicValue,
    pub(crate) preview: Option<RenderedImage>,
}

/// Central runtime state for the current editor. Off the serialized state.
#[derive(Default, Debug)]
pub(crate) struct RunState {
    nodes: HashMap<NodeId, NodeRunState>,
    pinned_outputs: BTreeMap<OutputAddress, PinnedOutputValue>,
    pinned_revision: u64,
    /// A run is in flight (`begin_run` → its `ExecutionFinished`). Drives the
    /// live repaint tick and whether the Cancel affordance shows.
    running: bool,
    /// RAM held by the worker's runtime cache after the last finished run
    /// (system RAM vs GPU VRAM), mirrored from its `ExecutionStats`. Drives the
    /// status bar's memory readout.
    pub(crate) cache_ram: RamUsage,
}

impl RunState {
    pub(crate) fn status(&self, id: NodeId) -> ExecStatus {
        self.nodes.get(&id).map(|n| n.status).unwrap_or_default()
    }

    /// Whether a run is in flight (kicked, not yet finished/cancelled). Drives
    /// a per-frame repaint so the running node's elapsed-so-far timer keeps
    /// ticking between progress events (a long single node emits none until it
    /// finishes), and gates the Cancel affordance.
    pub(crate) fn is_running(&self) -> bool {
        self.running
    }

    pub(crate) fn logs(&self, id: NodeId) -> &[LogEntry] {
        self.nodes
            .get(&id)
            .map(|n| n.logs.as_slice())
            .unwrap_or(&[])
    }

    /// This run's failure messages for a node (the errored node itself, or a
    /// composite instance aggregating its subtree). Empty unless it errored.
    pub(crate) fn errors(&self, id: NodeId) -> &[String] {
        self.nodes
            .get(&id)
            .map(|n| n.errors.as_slice())
            .unwrap_or(&[])
    }

    /// RAM this node's cached output holds after the last run (zero if it holds
    /// nothing). Read into the scene each rebuild to drive the node body's
    /// memory readout.
    pub(crate) fn ram(&self, id: NodeId) -> RamUsage {
        self.nodes.get(&id).map(|n| n.ram).unwrap_or_default()
    }

    /// A pinned output's latest pushed value, or `None` if it's never arrived
    /// (not pinned, or pinned but not yet run). Read by the canvas pin-
    /// preview widget ([`crate::gui::canvas::pin_ui::PinUi::draw`]) every
    /// frame.
    pub(crate) fn representative_pinned_output(
        &self,
        port: OutputPort,
    ) -> Option<&PinnedOutputValue> {
        self.pinned_outputs
            .iter()
            .filter(|(address, _)| {
                address.node.node_id == port.node_id && address.port_idx == port.port_idx
            })
            .max_by_key(|(_, value)| value.revision)
            .map(|(_, value)| value)
    }

    /// Fold a just-arrived pinned-output push onto its node: each pushed
    /// port's value replaces whatever this node held for that port before (a
    /// stale value from an earlier run, or nothing). A port not included in
    /// `pinned` is left untouched — e.g. a port unpinned mid-session still
    /// shows its last value until a whole-run failure [`clear`](Self::clear)s
    /// it.
    pub(crate) fn set_pinned_values(&mut self, pinned: PinnedOutputs) {
        if pinned.values.is_empty() {
            return;
        }
        self.pinned_revision = self
            .pinned_revision
            .checked_add(1)
            .expect("pinned-output revision overflow");
        let revision = self.pinned_revision;
        for output in pinned.values {
            let port_idx = output.port_idx;
            let value = output.value;
            let preview = value
                .as_custom::<LensImage>()
                .and_then(|_| convert_image_value(&value, PIN_PREVIEW_TEXTURE_DIM).ok());
            self.pinned_outputs.insert(
                OutputAddress {
                    node: pinned.node.clone(),
                    port_idx,
                },
                PinnedOutputValue {
                    revision,
                    value,
                    preview,
                },
            );
        }
    }

    /// Reproject a finished run's status + logs onto the per-node map.
    /// Status/logs are reset and rebuilt. Entries left carrying nothing are
    /// dropped. `flatten` is the compile-phase map of the program the stats
    /// came from (the host compiled, so the worker doesn't echo it back) —
    /// it projects the stats' flattened ids onto authoring nodes.
    pub(crate) fn set_results(&mut self, stats: &ExecutionStats, flatten: &FlattenMap) {
        self.running = false;
        self.cache_ram = stats.cache_ram;
        for node in self.nodes.values_mut() {
            node.status = ExecStatus::None;
            node.logs.clear();
            node.errors.clear();
            node.ram = RamUsage::default();
        }
        for e in &stats.executed_nodes {
            self.record_status(flatten, e.node_id, ExecStatus::Executed(e.elapsed_secs));
        }
        for id in &stats.cached_nodes {
            self.record_status(flatten, *id, ExecStatus::Cached);
        }
        for port in &stats.missing_inputs {
            self.record_status(flatten, port.node_id, ExecStatus::MissingInputs);
        }
        for e in &stats.node_errors {
            // A node cancelled mid-run didn't fail — it was interrupted and
            // will re-run next time. Leave it neutral (no glow) rather than
            // flagging it as an error.
            if matches!(e.error, RunError::Cancelled { .. }) {
                continue;
            }
            self.record_status(flatten, e.node_id, ExecStatus::Errored);
            self.record_error(flatten, e.node_id, &e.error);
        }
        for entry in &stats.logs {
            for node_id in flatten.attribution(entry.node_id) {
                self.nodes.entry(node_id).or_default().logs.push(LogEntry {
                    node_id,
                    level: entry.level,
                    message: entry.message.clone(),
                });
            }
        }
        // Per-node RAM folds like elapsed time: a flattened node's footprint adds
        // onto its authoring node and every enclosing composite instance, so an
        // instance aggregates its interior's memory.
        for node_ram in &stats.node_ram {
            for node_id in flatten.attribution(node_ram.node_id) {
                self.nodes.entry(node_id).or_default().ram += node_ram.usage;
            }
        }
        self.nodes
            .retain(|_, n| n.status != ExecStatus::None || !n.logs.is_empty() || n.ram.total() > 0);
    }

    /// Fold one flattened stat's `status` onto the node itself and every
    /// enclosing composite instance (via the flatten map's attribution).
    fn record_status(&mut self, flatten: &FlattenMap, flat_id: NodeId, status: ExecStatus) {
        for node_id in flatten.attribution(flat_id) {
            let slot = self.nodes.entry(node_id).or_default();
            slot.status = slot.status.merged(status);
        }
    }

    /// Fold one run error's message onto the errored node and every enclosing
    /// composite instance (same attribution as `record_status`), so the
    /// inspector can show the actual failure cause instead of a bare "errored".
    /// A subgraph instance accumulates its whole subtree's failures.
    fn record_error(&mut self, flatten: &FlattenMap, flat_id: NodeId, error: &RunError) {
        let message = error.to_string();
        for node_id in flatten.attribution(flat_id) {
            self.nodes
                .entry(node_id)
                .or_default()
                .errors
                .push(message.clone());
        }
    }

    /// Apply one live [`RunProgress`] event: mark the node(s) `Running` on
    /// `Started`, `Executed` on `Finished`. Overwrites the prior status (the
    /// final [`set_results`](Self::set_results) reconciles, e.g. summing an
    /// instance subtree's times) — deliberately *not* folded through
    /// `record_status`'s `merged`, since `Running` is live-only and must
    /// always win over a stale `Errored`/`MissingInputs` from the last run.
    /// `flatten` is the compile-phase map of the program the event came
    /// from (like [`set_results`](Self::set_results)) — `progress.node_id`
    /// is a flattened id, projected onto authoring nodes here.
    pub(crate) fn apply_progress(&mut self, progress: &RunProgress, flatten: &FlattenMap) {
        let status = match progress.phase {
            RunPhase::Started { at } => ExecStatus::Running(at),
            RunPhase::Finished { elapsed_secs } => ExecStatus::Executed(elapsed_secs),
        };
        for node_id in flatten.attribution(progress.node_id) {
            self.nodes.entry(node_id).or_default().status = status;
        }
    }

    /// Drop everything visible from a failed run: no glow, logs, or pinned
    /// values. The pinned revision stays monotonic so surviving derived view
    /// caches cannot mistake a later value for the cleared one.
    pub(crate) fn clear(&mut self) {
        self.running = false;
        self.nodes.clear();
        self.pinned_outputs.clear();
    }

    /// Mark a fresh run in flight while keeping status/logs/pinned values so
    /// the glow and pinned previews don't blank during compute; the new run's
    /// stats and pushes replace them as they arrive.
    pub(crate) fn begin_run(&mut self) {
        self.running = true;
        self.nodes
            .retain(|_, n| n.status != ExecStatus::None || !n.logs.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use scenarium::FlattenMapBuilder;
    use scenarium::FuncId;
    use scenarium::PinnedOutput;
    use scenarium::{ExecutedNodeStats, LogLevel, NodeError};

    fn nid(n: u128) -> NodeId {
        NodeId::from_u128(n)
    }

    /// Build an `ExecutionStats` with the given executed `(flat_id, secs)`
    /// and errored `flat_id`s. The flatten map isn't part of the stats —
    /// it's passed alongside to `set_results`, as in production.
    fn stats(executed: &[(NodeId, f64)], errored: &[NodeId]) -> ExecutionStats {
        ExecutionStats {
            elapsed_secs: 0.0,
            executed_nodes: executed
                .iter()
                .map(|&(node_id, elapsed_secs)| ExecutedNodeStats {
                    node_id,
                    elapsed_secs,
                })
                .collect(),
            missing_inputs: vec![],
            cached_nodes: vec![],
            triggered_events: vec![],
            node_errors: errored
                .iter()
                .map(|&node_id| NodeError {
                    node_id,
                    error: RunError::Invoke {
                        func_id: FuncId::from_u128(0),
                        message: "test error".into(),
                    },
                })
                .collect(),
            logs: vec![],
            cancelled: false,
            cache_ram: RamUsage::default(),
            node_ram: vec![],
        }
    }

    fn flatten_map(leaves: impl IntoIterator<Item = (NodeId, Vec<NodeId>, NodeId)>) -> FlattenMap {
        let mut builder = FlattenMapBuilder::new();
        for (flat_id, instances, node_id) in leaves {
            builder.insert_leaf(flat_id, instances, node_id);
        }
        builder.build()
    }

    #[test]
    fn apply_progress_marks_all_attributed_nodes_running_then_executed() {
        let (interior, instance) = (nid(1), nid(2));
        let flat = nid(100);
        let map = flatten_map([(flat, vec![instance], interior)]);

        let mut rs = RunState::default();

        // Started → every attributed node turns Running.
        rs.apply_progress(
            &RunProgress {
                node_id: flat,
                phase: RunPhase::Started {
                    at: std::time::Instant::now(),
                },
            },
            &map,
        );
        assert!(matches!(rs.status(interior), ExecStatus::Running(_)));
        assert!(matches!(rs.status(instance), ExecStatus::Running(_)));

        // Finished → Executed with the reported time (overwrites Running).
        rs.apply_progress(
            &RunProgress {
                node_id: flat,
                phase: RunPhase::Finished { elapsed_secs: 0.5 },
            },
            &map,
        );
        assert_eq!(rs.status(interior), ExecStatus::Executed(0.5));
        assert_eq!(rs.status(instance), ExecStatus::Executed(0.5));

        // A node no event mentioned stays None.
        assert_eq!(rs.status(nid(99)), ExecStatus::None);
    }

    /// Two instances of one def → the interior node's flattened ids both
    /// fold onto its authoring `interior` id, summing time; the instance
    /// nodes each get only their own run.
    #[test]
    fn aggregates_interior_across_instances_and_per_instance_subtree() {
        let interior = nid(1);
        let (inst_a, inst_b) = (nid(10), nid(20));
        let (flat_a, flat_b) = (nid(101), nid(102));

        let map = flatten_map([
            (flat_a, vec![inst_a], interior),
            (flat_b, vec![inst_b], interior),
        ]);

        let mut rs = RunState::default();
        rs.set_results(&stats(&[(flat_a, 2.0), (flat_b, 3.0)], &[]), &map);

        // Shared interior view: both instances' times sum (2 + 3).
        assert_eq!(rs.status(interior), ExecStatus::Executed(5.0));
        // Each instance node carries only its own run.
        assert_eq!(rs.status(inst_a), ExecStatus::Executed(2.0));
        assert_eq!(rs.status(inst_b), ExecStatus::Executed(3.0));
    }

    /// A node nested two levels deep accumulates onto *both* enclosing
    /// instances — the outer instance's total includes nested cost.
    #[test]
    fn outer_instance_total_includes_nested() {
        let interior = nid(1);
        let (outer, inner) = (nid(10), nid(20));
        let flat = nid(100);

        let map = flatten_map([(flat, vec![outer, inner], interior)]);

        let mut rs = RunState::default();
        rs.set_results(&stats(&[(flat, 4.0)], &[]), &map);

        assert_eq!(rs.status(interior), ExecStatus::Executed(4.0));
        assert_eq!(rs.status(inner), ExecStatus::Executed(4.0));
        assert_eq!(rs.status(outer), ExecStatus::Executed(4.0));
    }

    /// Worst status wins when a node both executed and errored (the
    /// errored node is in both lists); time is dropped with the upgrade.
    #[test]
    fn errored_beats_executed_on_same_node() {
        let interior = nid(1); // top-level: flattened id == interior
        let map = flatten_map([(interior, vec![], interior)]);

        let mut rs = RunState::default();
        rs.set_results(&stats(&[(interior, 1.0)], &[interior]), &map);
        assert_eq!(rs.status(interior), ExecStatus::Errored);
        // The failure message rides along with the status — the inspector
        // shows it instead of a bare "errored".
        assert_eq!(rs.errors(interior), ["test error"]);
    }

    /// A failure's message folds onto the errored interior node *and* its
    /// enclosing subgraph instance (same attribution as the status), while a
    /// `Cancelled` "error" records neither status nor message (a cancel is not
    /// a failure).
    #[test]
    fn error_messages_attribute_to_instance_and_skip_cancelled() {
        let interior = nid(1);
        let cancelled_interior = nid(2);
        let inst = nid(10);
        let (fail_flat, cancel_flat) = (nid(100), nid(101));
        let map = flatten_map([
            (fail_flat, vec![inst], interior),
            (cancel_flat, vec![inst], cancelled_interior),
        ]);

        let node_err = |node_id, error| NodeError { node_id, error };
        let mut s = stats(&[], &[]);
        s.node_errors = vec![
            node_err(
                fail_flat,
                RunError::Invoke {
                    func_id: FuncId::from_u128(0),
                    message: "no light frames provided".into(),
                },
            ),
            node_err(
                cancel_flat,
                RunError::Cancelled {
                    func_id: FuncId::from_u128(0),
                },
            ),
        ];

        let mut rs = RunState::default();
        rs.set_results(&s, &map);

        // The real failure paints both the interior and the enclosing instance.
        assert_eq!(rs.status(interior), ExecStatus::Errored);
        assert_eq!(rs.errors(interior), ["no light frames provided"]);
        assert_eq!(rs.status(inst), ExecStatus::Errored);
        assert_eq!(rs.errors(inst), ["no light frames provided"]);
        // The cancelled node contributes no extra status/message (only the one
        // real failure's message is present, not a second entry for the cancel).
        assert_eq!(rs.errors(interior).len(), 1, "cancel adds no message");
        assert_eq!(rs.status(cancelled_interior), ExecStatus::None);
        assert!(rs.errors(cancelled_interior).is_empty());
    }

    /// A log line emitted inside a subgraph instance attributes to both
    /// the interior node and the enclosing instance, preserving level +
    /// message, re-keyed to each editor node.
    #[test]
    fn set_results_attributes_logs_to_interior_and_instance() {
        let interior = nid(1);
        let inst = nid(10);
        let flat = nid(100);
        let map = flatten_map([(flat, vec![inst], interior)]);

        let mut s = stats(&[], &[]);
        s.logs.push(LogEntry {
            node_id: flat,
            level: LogLevel::Warn,
            message: "hi".into(),
        });

        let mut rs = RunState::default();
        rs.set_results(&s, &map);

        let i = rs.logs(interior);
        assert_eq!(i.len(), 1, "interior carries the line");
        assert_eq!(i[0].message, "hi");
        assert_eq!(i[0].level, LogLevel::Warn);
        assert_eq!(i[0].node_id, interior);
        let n = rs.logs(inst);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].node_id, inst, "re-keyed to the instance");
    }

    /// A fresh run keeps status/logs so the glow survives a recompute.
    #[test]
    fn begin_run_keeps_status() {
        let node = nid(1);
        let map = flatten_map([(node, vec![], node)]);

        let mut rs = RunState::default();
        rs.set_results(&stats(&[(node, 1.0)], &[]), &map);

        assert!(!rs.is_running(), "not running after a finished run");
        rs.begin_run();

        assert!(rs.is_running(), "running once a run is kicked");
        assert_eq!(rs.status(node), ExecStatus::Executed(1.0), "status lingers");

        // The finishing run clears the in-flight flag.
        rs.set_results(&stats(&[], &[]), &FlattenMap::default());
        assert!(!rs.is_running(), "not running after results land");
    }

    /// A pinned-output push is retrievable by node + port, and a second push
    /// for the same port replaces the first rather than appending.
    #[test]
    fn set_pinned_values_stores_and_replaces_per_port() {
        use scenarium::StaticValue;

        let node = nid(1);
        let mut rs = RunState::default();
        assert!(
            rs.representative_pinned_output(OutputPort::new(node, 0))
                .is_none(),
            "nothing pushed yet"
        );

        rs.set_pinned_values(PinnedOutputs {
            node: NodeAddress::root(node),
            values: vec![PinnedOutput {
                port_idx: 0,
                value: DynamicValue::Static(StaticValue::Int(7)),
            }],
        });
        let first = rs
            .representative_pinned_output(OutputPort::new(node, 0))
            .unwrap();
        assert_eq!(first.value.as_i64(), Some(7), "first push is stored");
        let first_revision = first.revision;

        rs.set_pinned_values(PinnedOutputs {
            node: NodeAddress::root(node),
            values: vec![PinnedOutput {
                port_idx: 0,
                value: DynamicValue::Static(StaticValue::Int(8)),
            }],
        });
        let replacement = rs
            .representative_pinned_output(OutputPort::new(node, 0))
            .unwrap();
        assert_eq!(
            replacement.value.as_i64(),
            Some(8),
            "a fresh push for the same port replaces it, not appends"
        );
        assert!(
            replacement.revision > first_revision,
            "a fresh push advances the view-facing revision"
        );

        let desc = imaginarium::ImageDesc::new(512, 256, imaginarium::ColorFormat::RGBA_U8);
        let raw = imaginarium::Image::new_with_data(desc, vec![128; 512 * 256 * 4]).unwrap();
        let value =
            DynamicValue::from_custom(LensImage::new(imaginarium::ImageBuffer::from_cpu(raw)));
        rs.set_pinned_values(PinnedOutputs {
            node: NodeAddress::root(node),
            values: vec![PinnedOutput { port_idx: 1, value }],
        });

        let image = rs
            .representative_pinned_output(OutputPort::new(node, 1))
            .unwrap()
            .preview
            .as_ref()
            .expect("image preview is prepared while ingesting the push");
        assert_eq!(image.native_size, glam::UVec2::new(512, 256));
        assert_eq!(image.native_format, imaginarium::ColorFormat::RGBA_U8);
        assert_eq!(image.image.width, 256);
        assert_eq!(image.image.height, 128);
        assert_eq!(image.image.pixels, vec![128; 256 * 128 * 4]);
    }

    #[test]
    fn set_pinned_values_keeps_subgraph_instances_separate() {
        use scenarium::StaticValue;

        let node = nid(1);
        let instance_a = nid(2);
        let instance_b = nid(3);
        let address_a = NodeAddress {
            instances: vec![instance_a],
            node_id: node,
        };
        let address_b = NodeAddress {
            instances: vec![instance_b],
            node_id: node,
        };
        let mut rs = RunState::default();

        rs.set_pinned_values(PinnedOutputs {
            node: address_a.clone(),
            values: vec![PinnedOutput {
                port_idx: 0,
                value: DynamicValue::Static(StaticValue::Int(7)),
            }],
        });
        rs.set_pinned_values(PinnedOutputs {
            node: address_b.clone(),
            values: vec![PinnedOutput {
                port_idx: 0,
                value: DynamicValue::Static(StaticValue::Int(8)),
            }],
        });

        assert_eq!(rs.pinned_outputs.len(), 2);
        assert_eq!(
            rs.pinned_outputs[&OutputAddress {
                node: address_a,
                port_idx: 0,
            }]
                .value
                .as_i64(),
            Some(7)
        );
        assert_eq!(
            rs.pinned_outputs[&OutputAddress {
                node: address_b,
                port_idx: 0,
            }]
                .value
                .as_i64(),
            Some(8)
        );
        assert_eq!(
            rs.representative_pinned_output(OutputPort::new(node, 0))
                .unwrap()
                .value
                .as_i64(),
            Some(8),
            "the latest instance push is the representative"
        );
    }

    /// `clear` (a whole-run failure) drops pinned values along with
    /// everything else.
    #[test]
    fn clear_drops_pinned_values() {
        use scenarium::StaticValue;

        let node = nid(1);
        let mut rs = RunState::default();
        rs.set_pinned_values(PinnedOutputs {
            node: NodeAddress::root(node),
            values: vec![PinnedOutput {
                port_idx: 0,
                value: DynamicValue::Static(StaticValue::Int(7)),
            }],
        });
        let revision = rs
            .representative_pinned_output(OutputPort::new(node, 0))
            .unwrap()
            .revision;
        rs.clear();
        assert!(
            rs.representative_pinned_output(OutputPort::new(node, 0))
                .is_none()
        );

        rs.set_pinned_values(PinnedOutputs {
            node: NodeAddress::root(node),
            values: vec![PinnedOutput {
                port_idx: 0,
                value: DynamicValue::Static(StaticValue::Int(8)),
            }],
        });
        assert!(
            rs.representative_pinned_output(OutputPort::new(node, 0))
                .unwrap()
                .revision
                > revision,
            "clear does not recycle a revision a surviving view may cache"
        );
    }

    /// A new epoch keeps a pinned value alive even when its node carries no
    /// status/logs — the same "don't blank during compute" guarantee
    /// `begin_run` already gives status/logs.
    #[test]
    fn begin_run_keeps_pinned_values_with_no_other_state() {
        use scenarium::StaticValue;

        let node = nid(1);
        let mut rs = RunState::default();
        rs.set_pinned_values(PinnedOutputs {
            node: NodeAddress::root(node),
            values: vec![PinnedOutput {
                port_idx: 0,
                value: DynamicValue::Static(StaticValue::Int(7)),
            }],
        });
        assert_eq!(rs.status(node), ExecStatus::None, "no status ever set");

        rs.begin_run();

        assert_eq!(
            rs.representative_pinned_output(OutputPort::new(node, 0))
                .unwrap()
                .value
                .as_i64(),
            Some(7),
            "pinned value survives an epoch bump with nothing else to anchor it"
        );
    }

    /// `set_results`'s reprojection doesn't prune a node whose only surviving
    /// fact is a pinned value (e.g. it was cut/pruned this run and
    /// contributes to none of the run's stat lists).
    #[test]
    fn set_results_keeps_a_node_holding_only_a_pinned_value() {
        use scenarium::StaticValue;

        let node = nid(1);
        let mut rs = RunState::default();
        rs.set_pinned_values(PinnedOutputs {
            node: NodeAddress::root(node),
            values: vec![PinnedOutput {
                port_idx: 0,
                value: DynamicValue::Static(StaticValue::Int(7)),
            }],
        });

        // A run that says nothing at all about `node` (no executed/cached/
        // errored entry) would otherwise reset it to a prunable `None`.
        rs.set_results(&stats(&[], &[]), &FlattenMap::default());

        assert_eq!(
            rs.representative_pinned_output(OutputPort::new(node, 0))
                .unwrap()
                .value
                .as_i64(),
            Some(7),
            "pinned value isn't dropped just because this run didn't mention the node"
        );
    }
}
