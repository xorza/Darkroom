//! Per-frame node geometry: a galley cache (the only expensive part of node
//! layout) plus a pure value type computed on demand from cached galleys +
//! position + scale. `GraphLayout` is the cross-frame piece; `NodeLayout`
//! is freshly computed at every call site.

use common::BoolExt;
use common::key_index_vec::{KeyIndexKey, KeyIndexVec};
use eframe::egui;
use egui::{FontId, Galley, Pos2, Rect, Vec2, pos2, vec2};
use scenarium::graph::NodeId;
use scenarium::prelude::{Func, FuncBehavior};
use std::sync::Arc;

use crate::common::UiEquals;
use crate::gui::graph_ui::ctx::GraphContext;
use crate::gui::graph_ui::gesture::Gesture;
use crate::gui::graph_ui::port::{PortKind, PortRef};
use crate::gui::style::Style;
use crate::gui::{Gui, ViewParams};

// ============================================================================
// NodeGalleys — cached font layouts
// ============================================================================
//
// Galleys live behind egui's internal `GalleyCache`, so the actual text
// shaping/layout is already amortized across frames. What this cache
// saves is the per-call dispatch overhead — `String` alloc, LayoutJob
// construction, hash, IntMap lookup, `Arc::clone` — for graph nodes
// whose name and scale rarely change. At ~3-5 µs per dispatched call
// times N nodes times ports/node, this matters at graph scale (hundreds
// of nodes); for tens of nodes it's noise.
//
// Storing the `Arc<Galley>` here also keeps `NodeLayout::compute` pure:
// it reads `.size()` off cached galleys and the cached `sub_row_height`
// without touching `Gui` itself. Everything else (rects, port centers)
// is cheap enough to compute from scratch each frame — see `NodeLayout`.

#[derive(Debug)]
pub struct NodeGalleys {
    scale: f32,
    pub title: Arc<Galley>,
    pub inputs: Vec<Arc<Galley>>,
    pub outputs: Vec<Arc<Galley>>,
    pub events: Vec<Arc<Galley>>,
    /// Row height of `style.sub_font` at the current scale — cached
    /// here so `NodeLayout::compute` can stay pure (no `&mut Gui`).
    pub sub_row_height: f32,
}

impl NodeGalleys {
    pub fn new(gui: &mut Gui<'_>, func: &Func, node_name: &str) -> Self {
        let mut this = Self {
            scale: gui.scale(),
            title: Self::make_title(gui, node_name),
            inputs: Vec::new(),
            outputs: Vec::new(),
            events: Vec::new(),
            sub_row_height: sub_row_height(gui),
        };
        this.rebuild_port_galleys(gui, func);
        this
    }

    /// Refresh cached galleys if the node name or GUI scale changed.
    pub fn update(&mut self, gui: &mut Gui<'_>, func: &Func, node_name: &str) {
        let scale_changed = !self.scale.ui_equals(gui.scale());

        if self.title.text() != node_name || scale_changed {
            self.title = Self::make_title(gui, node_name);
        }

        if scale_changed {
            self.rebuild_port_galleys(gui, func);
            self.sub_row_height = sub_row_height(gui);
            self.scale = gui.scale();
        }
    }

    fn rebuild_port_galleys(&mut self, gui: &Gui<'_>, func: &Func) {
        let font = &gui.style.sub_font;
        let collect = |names: &mut dyn Iterator<Item = &str>| -> Vec<Arc<Galley>> {
            names.map(|n| Self::make_sub(gui, n, font)).collect()
        };
        self.inputs = collect(&mut func.inputs.iter().map(|p| p.name.as_str()));
        self.outputs = collect(&mut func.outputs.iter().map(|p| p.name.as_str()));
        self.events = collect(&mut func.events.iter().map(|e| e.name.as_str()));
    }

    fn make_title(gui: &Gui<'_>, text: &str) -> Arc<Galley> {
        gui.layout_no_wrap(text, &gui.style.body_font, gui.style.text_color)
    }

    fn make_sub(gui: &Gui<'_>, text: &str, font: &FontId) -> Arc<Galley> {
        gui.layout_no_wrap(text, font, gui.style.text_color)
    }
}

fn sub_row_height(gui: &mut Gui<'_>) -> f32 {
    gui.font_height(&gui.style.sub_font.clone())
}

// ============================================================================
// NodeLayout — pure per-frame geometry
// ============================================================================
//
// Plain data produced by `compute`, materialized on demand by
// `GraphLayout::node_layout`. No caching, no mutation.

#[derive(Debug, Clone, Copy)]
pub struct NodeLayout {
    pub body_rect: Rect,
    pub remove_btn_rect: Rect,
    pub has_cache_btn: bool,
    pub cache_button_rect: Rect,
    pub status_dot_center: Pos2,
    pub input_first_center: Pos2,
    pub output_first_center: Pos2,
    pub event_first_center: Pos2,
    pub port_row_height: f32,
    pub port_activation_radius: f32,
    pub header_row_height: f32,
}

impl NodeLayout {
    /// Pure function: returns the full geometry for a node given its
    /// cached galleys, function signature, and world-space position.
    pub fn compute(
        galleys: &NodeGalleys,
        func: &Func,
        vp: &ViewParams,
        origin: Pos2,
        node_pos: Pos2,
    ) -> Self {
        let style: &Style = &vp.style;
        let scale = vp.scale;
        let padding = style.padding;
        let small_padding = style.small_padding;
        let remove_btn_size = style.node.remove_btn_size;
        let status_dot_radius = style.node.status_dot_radius;
        let port_label_side_padding = style.node.port_label_side_padding;
        let port_radius = style.node.port_radius;
        let cache_btn_width = style.node.cache_btn_width;
        let sub_font_size = style.sub_font.size;
        let row_height = galleys.sub_row_height + small_padding;

        // Header dimensions
        let remove_size = remove_btn_size + small_padding * 2.0;
        let status_dot_size = status_dot_radius * 2.0;
        let header_height = galleys.title.size().y.max(remove_size).max(status_dot_size);

        let title_width = galleys.title.size().x + padding * 2.0;
        let status_width = 2.0 * (small_padding + status_dot_size);
        let header_width = title_width + padding + status_width + padding + remove_size + padding;

        // Port row dimensions
        let row_count = galleys
            .inputs
            .len()
            .max(galleys.outputs.len() + galleys.events.len())
            .max(1);

        let max_left = galleys
            .inputs
            .iter()
            .map(|g| g.size().x)
            .fold(0.0_f32, f32::max);
        let max_right = galleys
            .outputs
            .iter()
            .chain(&galleys.events)
            .map(|g| g.size().x)
            .fold(0.0_f32, f32::max);
        let row_width = port_label_side_padding * 2.0
            + max_left
            + max_right
            + (max_left > 0.0 && max_right > 0.0).then_else(padding, 0.0);

        // Cache button
        let has_cache_btn = !(func.terminal
            || func.outputs.is_empty()
            || (func.behavior == FuncBehavior::Pure && func.inputs.is_empty()));
        let cache_row_height = if has_cache_btn {
            sub_font_size + padding * 2.0
        } else {
            0.0
        };

        // Final dimensions
        let header_row_height = header_height + small_padding * 2.0;
        let port_row_height = row_height.max(port_radius * 2.0);

        let node_width = header_width.max(row_width).max(80.0 * scale);
        let node_height = header_row_height
            + cache_row_height
            + port_row_height * row_count as f32
            + padding * 2.0;

        let body_local = Rect::from_min_size(Pos2::ZERO, vec2(node_width, node_height));

        let remove_local = Rect::from_min_size(
            pos2(
                body_local.max.x - padding - remove_size,
                body_local.min.y + padding,
            ),
            Vec2::splat(remove_size),
        );

        let dot_first_local = pos2(
            remove_local.min.x - padding - status_dot_radius,
            header_row_height * 0.5,
        );

        let cache_button_local = Rect::from_min_size(
            pos2(padding, header_row_height + padding),
            vec2(cache_btn_width, sub_font_size),
        );

        let port_base_y = header_row_height + cache_row_height + padding + port_row_height * 0.5;
        let input_first_local = pos2(0.0, port_base_y);
        let output_first_local = pos2(node_width, port_base_y);
        let event_first_local = pos2(
            node_width,
            port_base_y + port_row_height * galleys.outputs.len() as f32,
        );

        let global_offset = (origin + node_pos.to_vec2() * scale).to_vec2();

        Self {
            body_rect: body_local.translate(global_offset),
            remove_btn_rect: remove_local.translate(global_offset),
            has_cache_btn,
            cache_button_rect: cache_button_local.translate(global_offset),
            status_dot_center: dot_first_local + global_offset,
            input_first_center: input_first_local + global_offset,
            output_first_center: output_first_local + global_offset,
            event_first_center: event_first_local + global_offset,
            port_row_height,
            port_activation_radius: port_row_height * 0.5,
            header_row_height,
        }
    }

    // === Port position accessors ===

    pub fn input_center(&self, index: usize) -> Pos2 {
        self.port_at_row(self.input_first_center, index)
    }

    pub fn output_center(&self, index: usize) -> Pos2 {
        self.port_at_row(self.output_first_center, index)
    }

    pub fn event_center(&self, index: usize) -> Pos2 {
        self.port_at_row(self.event_first_center, index)
    }

    pub fn trigger_center(&self) -> Pos2 {
        self.body_rect.min
    }

    pub fn port_center(&self, port: &PortRef) -> Pos2 {
        match port.kind {
            PortKind::Input => self.input_center(port.port_idx),
            PortKind::Output => self.output_center(port.port_idx),
            PortKind::Event => self.event_center(port.port_idx),
            PortKind::Trigger => self.trigger_center(),
        }
    }

    fn port_at_row(&self, base: Pos2, row: usize) -> Pos2 {
        pos2(base.x, base.y + self.port_row_height * row as f32)
    }
}

// ============================================================================
// GraphLayout — per-node frame state
// ============================================================================

/// Galleys + computed layout for one node. Galleys are cross-frame
/// state (rebuilt only on name/scale change); layout is per-frame
/// (recomputed every frame — pure arithmetic). Co-located so a
/// single `by_key` lookup yields both, and `view_nodes` is iterated
/// once per frame to refresh both.
#[derive(Debug)]
pub struct NodeFrame {
    node_id: NodeId,
    pub galleys: NodeGalleys,
    pub layout: NodeLayout,
}

impl KeyIndexKey<NodeId> for NodeFrame {
    fn key(&self) -> &NodeId {
        &self.node_id
    }
}

#[derive(Debug, Default)]
pub struct GraphLayout {
    nodes: KeyIndexVec<NodeId, NodeFrame>,
}

/// World-space origin (where graph coord (0, 0) lands on screen) for
/// the current frame. Derived; kept out of `GraphLayout` so there's
/// no cached-then-stale foot-gun.
pub fn origin(vp: &ViewParams, ctx: &GraphContext<'_>) -> Pos2 {
    vp.rect.min + ctx.view_graph.pan
}

impl GraphLayout {
    /// Per-frame refresh: rebuild stale galleys (name/scale change),
    /// insert new entries for newly-added nodes, and recompute every
    /// node's layout from the gesture's current drag offset (zero
    /// except for the node being dragged). Run once per frame at the
    /// top of the content phase.
    pub fn refresh(&mut self, gui: &mut Gui<'_>, ctx: &GraphContext<'_>, gesture: &Gesture) {
        let vp = gui.view_params();
        let origin = origin(&vp, ctx);
        let mut compact = self.nodes.compact_insert_start();
        for view_node in ctx.view_graph.view_nodes.iter() {
            let node_id = view_node.id;
            let node = ctx.view_graph.graph.by_id(&node_id).unwrap();
            let func = ctx.func_lib.by_id(&node.func_id).unwrap();
            let drag_offset = gesture.node_drag_offset_for(&node_id);
            let pos = view_node.pos + drag_offset;

            let (_, frame) = compact.insert_with(&node_id, || {
                let galleys = NodeGalleys::new(gui, func, &node.name);
                let layout = NodeLayout::compute(&galleys, func, &vp, origin, pos);
                NodeFrame {
                    node_id,
                    galleys,
                    layout,
                }
            });
            frame.galleys.update(gui, func, &node.name);
            frame.layout = NodeLayout::compute(&frame.galleys, func, &vp, origin, pos);
        }
    }

    /// After `handle_node_interactions` accumulates this frame's
    /// drag delta, recompute the dragged node's layout so the
    /// connection and body-render passes see the up-to-date position.
    /// No-op when no node drag is in flight.
    pub fn refresh_dragged_layout(
        &mut self,
        vp: &ViewParams,
        ctx: &GraphContext<'_>,
        gesture: &Gesture,
    ) {
        let Some(drag) = gesture.node_drag() else {
            return;
        };
        let view_node = ctx.view_graph.view_nodes.by_key(&drag.node_id).unwrap();
        let node = ctx.view_graph.graph.by_id(&drag.node_id).unwrap();
        let func = ctx.func_lib.by_id(&node.func_id).unwrap();
        let frame = self.nodes.by_key_mut(&drag.node_id).unwrap();
        frame.layout = NodeLayout::compute(
            &frame.galleys,
            func,
            vp,
            origin(vp, ctx),
            view_node.pos + drag.offset,
        );
    }

    pub fn frame(&self, node_id: &NodeId) -> &NodeFrame {
        self.nodes.by_key(node_id).unwrap()
    }

    /// Convenience: read this frame's layout for a node. Equivalent
    /// to `frame(node_id).layout`.
    pub fn cached_layout(&self, node_id: &NodeId) -> &NodeLayout {
        &self.frame(node_id).layout
    }

    /// Convenience: read this frame's galleys for a node. Equivalent
    /// to `&frame(node_id).galleys`.
    pub fn node_galleys(&self, node_id: &NodeId) -> &NodeGalleys {
        &self.frame(node_id).galleys
    }

    /// Compute geometry for a single node fresh, outside the
    /// per-frame cache lifecycle. Used by the overlay buttons
    /// (fit-all, view-selected) which compute targets at click time.
    pub fn node_layout(
        &self,
        vp: &ViewParams,
        ctx: &GraphContext<'_>,
        node_id: &NodeId,
        drag_offset: Vec2,
    ) -> NodeLayout {
        let frame = self.frame(node_id);
        let node = ctx.view_graph.graph.by_id(node_id).unwrap();
        let func = ctx.func_lib.by_id(&node.func_id).unwrap();
        let view_node = ctx.view_graph.view_nodes.by_key(node_id).unwrap();
        NodeLayout::compute(
            &frame.galleys,
            func,
            vp,
            origin(vp, ctx),
            view_node.pos + drag_offset,
        )
    }
}
