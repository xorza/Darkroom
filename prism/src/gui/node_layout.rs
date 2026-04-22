use common::BoolExt;
use common::key_index_vec::KeyIndexKey;
use eframe::egui;
use egui::{FontId, Galley, Pos2, Rect, Vec2, pos2, vec2};
use scenarium::graph::NodeId;
use scenarium::prelude::{Func, FuncBehavior};
use std::sync::Arc;

use crate::common::UiEquals;
use crate::gui::Gui;
use crate::gui::connection_ui::PortKind;
use crate::gui::graph_layout::PortRef;

// ============================================================================
// NodeGalleys — cached font layouts
// ============================================================================
//
// Galleys are the only expensive part of per-node layout: each one is a
// shaped + laid-out text allocation owned by `Arc<Galley>`. They change
// only when the node name or the GUI scale changes, so they're cached
// here and rebuilt lazily. Everything else (rects, port centers) is
// cheap enough to compute from scratch each frame — see `NodeLayout`.

#[derive(Debug)]
pub struct NodeGalleys {
    node_id: NodeId,
    scale: f32,
    pub title: Arc<Galley>,
    pub inputs: Vec<Arc<Galley>>,
    pub outputs: Vec<Arc<Galley>>,
    pub events: Vec<Arc<Galley>>,
}

impl NodeGalleys {
    pub fn new(gui: &Gui<'_>, node_id: NodeId, func: &Func, node_name: &str) -> Self {
        let mut this = Self {
            node_id,
            scale: gui.scale(),
            title: Self::make_title(gui, node_name),
            inputs: Vec::new(),
            outputs: Vec::new(),
            events: Vec::new(),
        };
        this.rebuild_port_galleys(gui, func);
        this
    }

    /// Refresh cached galleys if the node name or GUI scale changed.
    pub fn update(&mut self, gui: &Gui<'_>, func: &Func, node_name: &str) {
        let scale_changed = !self.scale.ui_equals(gui.scale());

        if self.title.text() != node_name || scale_changed {
            self.title = Self::make_title(gui, node_name);
        }

        if scale_changed {
            self.rebuild_port_galleys(gui, func);
            self.scale = gui.scale();
        }
    }

    fn rebuild_port_galleys(&mut self, gui: &Gui<'_>, func: &Func) {
        let font = &gui.style.sub_font;
        self.inputs = func
            .inputs
            .iter()
            .map(|p| Self::make_sub(gui, &p.name, font))
            .collect();
        self.outputs = func
            .outputs
            .iter()
            .map(|p| Self::make_sub(gui, &p.name, font))
            .collect();
        self.events = func
            .events
            .iter()
            .map(|e| Self::make_sub(gui, &e.name, font))
            .collect();
    }

    fn make_title(gui: &Gui<'_>, text: &str) -> Arc<Galley> {
        gui.painter().layout_no_wrap(
            text.to_string(),
            gui.style.body_font.clone(),
            gui.style.text_color,
        )
    }

    fn make_sub(gui: &Gui<'_>, text: &str, font: &FontId) -> Arc<Galley> {
        gui.painter()
            .layout_no_wrap(text.to_string(), font.clone(), gui.style.text_color)
    }
}

impl KeyIndexKey<NodeId> for NodeGalleys {
    fn key(&self) -> &NodeId {
        &self.node_id
    }
}

// ============================================================================
// NodeLayout — pure per-frame geometry
// ============================================================================
//
// Plain data. Recomputed each frame from `NodeGalleys` + `ViewNode.pos`
// + drag offset + style. Kept in `GraphLayout`'s cache only so that
// the next frame's interaction pass can read the previous frame's
// `body_rect` without a round-trip through egui's memory.

#[derive(Debug, Clone, Copy)]
pub struct NodeLayout {
    pub node_id: NodeId,
    pub body_rect: Rect,
    pub remove_btn_rect: Rect,
    pub has_cache_btn: bool,
    pub cache_button_rect: Rect,
    pub dot_first_center: Pos2,
    pub input_first_center: Pos2,
    pub output_first_center: Pos2,
    pub port_row_height: f32,
    pub port_activation_radius: f32,
    pub header_row_height: f32,
    pub input_count: usize,
    pub output_count: usize,
    pub event_count: usize,
}

impl NodeLayout {
    /// Pure function: returns the full geometry for a node given its
    /// cached galleys, function signature, and world-space position.
    pub fn compute(
        node_id: NodeId,
        galleys: &NodeGalleys,
        func: &Func,
        gui: &mut Gui<'_>,
        origin: Pos2,
        node_pos: Pos2,
    ) -> Self {
        let scale = gui.scale();
        let padding = gui.style.padding;
        let small_padding = gui.style.small_padding;
        let remove_btn_size = gui.style.node.remove_btn_size;
        let status_dot_radius = gui.style.node.status_dot_radius;
        let port_label_side_padding = gui.style.node.port_label_side_padding;
        let port_radius = gui.style.node.port_radius;
        let cache_btn_width = gui.style.node.cache_btn_width;
        let sub_font = gui.style.sub_font.clone();
        let sub_font_size = sub_font.size;
        let row_height = gui.font_height(&sub_font) + small_padding;

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

        let (max_left, max_right) = compute_max_galley_widths(galleys, row_count);
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

        let global_offset = (origin + node_pos.to_vec2() * scale).to_vec2();

        Self {
            node_id,
            body_rect: body_local.translate(global_offset),
            remove_btn_rect: remove_local.translate(global_offset),
            has_cache_btn,
            cache_button_rect: cache_button_local.translate(global_offset),
            dot_first_center: dot_first_local + global_offset,
            input_first_center: input_first_local + global_offset,
            output_first_center: output_first_local + global_offset,
            port_row_height,
            port_activation_radius: port_row_height * 0.5,
            header_row_height,
            input_count: galleys.inputs.len(),
            output_count: galleys.outputs.len(),
            event_count: galleys.events.len(),
        }
    }

    // === Port position accessors ===

    pub fn input_center(&self, index: usize) -> Pos2 {
        debug_assert!(index < self.input_count);
        self.port_at_row(self.input_first_center, index)
    }

    pub fn output_center(&self, index: usize) -> Pos2 {
        debug_assert!(index < self.output_count);
        self.port_at_row(self.output_first_center, index)
    }

    pub fn event_center(&self, index: usize) -> Pos2 {
        debug_assert!(index < self.event_count);
        self.port_at_row(self.output_first_center, index + self.output_count)
    }

    pub fn trigger_center(&self) -> Pos2 {
        self.body_rect.min
    }

    pub fn dot_center(&self, index: usize, dot_step: f32) -> Pos2 {
        pos2(
            self.dot_first_center.x - dot_step * index as f32,
            self.dot_first_center.y,
        )
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

impl KeyIndexKey<NodeId> for NodeLayout {
    fn key(&self) -> &NodeId {
        &self.node_id
    }
}

fn compute_max_galley_widths(galleys: &NodeGalleys, row_count: usize) -> (f32, f32) {
    let mut max_left: f32 = 0.0;
    let mut max_right: f32 = 0.0;

    for row in 0..row_count {
        let left = galleys.inputs.get(row).map_or(0.0, |g| g.size().x);

        let right = if row < galleys.outputs.len() {
            galleys.outputs.get(row).map_or(0.0, |g| g.size().x)
        } else {
            let event_row = row - galleys.outputs.len();
            galleys.events.get(event_row).map_or(0.0, |g| g.size().x)
        };

        max_left = max_left.max(left);
        max_right = max_right.max(right);
    }

    (max_left, max_right)
}
