use glam::Vec2;
use scenarium::prelude::NodeId;
use std::collections::HashMap;

use crate::gui::node_widget::NodePortSpans;

/// Interframe scratch shared across darkroom's render pipeline. One
/// instance lives on `App`; `view::build` reads the previous frame's
/// snapshot and refills it for next frame. Adding a new cross-frame
/// cache = new field here + extend [`Self::clear`].
#[derive(Default)]
pub struct FrameCache {
    pub ports: PortCache,
}

impl FrameCache {
    /// Drop all retained data length-wise; capacities are kept so the
    /// next frame's refill is allocation-free.
    pub fn clear(&mut self) {
        self.ports.clear();
    }
}

/// Port centers captured at the end of frame N and consumed at the
/// start of frame N+1 — lets `view::build` draw connections *before*
/// nodes (so beziers land behind node bodies) while still threading
/// the real laid-out port centers through.
///
/// Flat layout: `centers` pools all `Vec2`s in node-then-input-then-output
/// order; `nodes` maps a `NodeId` to the pair of `PortSpan`s slicing into
/// the pool. A node only earns an entry once every one of its ports
/// resolved a layout rect (frame 2+); first-frame nodes are absent from
/// `nodes`, so `draw_connections` skips them via `nodes.get(&id)`.
#[derive(Default)]
pub struct PortCache {
    pub centers: Vec<Vec2>,
    pub nodes: HashMap<NodeId, NodePortSpans>,
}

impl PortCache {
    pub fn clear(&mut self) {
        self.centers.clear();
        self.nodes.clear();
    }
}
