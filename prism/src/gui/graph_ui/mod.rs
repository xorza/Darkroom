//! Top-level graph view. Owns the per-frame render pipeline
//! (content → overlays → zoom/pan) and the interaction state machine.
//!
//! The heavy logic lives in submodules:
//! - `connections` — drag/snap/commit for data + event wires, breaker
//! - `overlays`    — button bars, new-node popup, const-bind routing
//! - `pan_zoom`    — viewport transforms and fit/reset targets

use common::BoolExt;
use eframe::egui;
use egui::{Pos2, Rect, Response, Sense, StrokeKind, Vec2};

use crate::common::StableId;
use crate::gui::Gui;
use crate::gui::graph_ui::background::GraphBackgroundRenderer;
use crate::gui::graph_ui::connections::ConnectionUi;
use crate::gui::graph_ui::ctx::GraphContext;
use crate::gui::graph_ui::frame_output::FrameOutput;
use crate::gui::graph_ui::gesture::Gesture;
use crate::gui::graph_ui::layout::GraphLayout;
use crate::gui::graph_ui::nodes::NodeUi;
use crate::gui::graph_ui::nodes::details::NodeDetailsUi;
use crate::gui::graph_ui::nodes::new_node::NewNodeUi;
use crate::gui::widgets::HitRegion;
use crate::input::InputSnapshot;
use crate::model::ArgumentValuesCache;
use crate::model::argument_values_cache::CacheEvent;

pub mod background;
pub mod connections;
pub mod ctx;
pub mod frame_output;
pub mod gesture;
pub mod layout;
pub mod nodes;
mod overlays;
mod pan_zoom;
pub mod port;

pub(crate) use connections::handlers::ConnectionError;
use pan_zoom::{fit_all_nodes_target, view_selected_node_target};

pub(crate) const MIN_ZOOM: f32 = 0.2;
pub(crate) const MAX_ZOOM: f32 = 4.0;
pub(crate) const WHEEL_ZOOM_SPEED: f32 = 0.08;

/// View-control button clicks are mutually exclusive (the user can
/// only click one button per frame). Encoded as an `Option<enum>`
/// instead of three booleans so the compiler enforces that.
#[derive(Debug)]
pub(super) enum ViewButtonAction {
    FitAll,
    ViewSelected,
    ResetView,
}

#[derive(Debug)]
pub(super) struct ButtonResult {
    pub(super) response: Response,
    pub(super) action: Option<ViewButtonAction>,
}

#[derive(Debug, Default)]
pub struct GraphUi {
    /// Centralized interaction state machine.
    gesture: Gesture,
    connections: ConnectionUi,
    graph_layout: GraphLayout,
    node_ui: NodeUi,
    dots_background: GraphBackgroundRenderer,
    new_node_ui: NewNodeUi,
    node_details_ui: NodeDetailsUi,
    /// UI-owned per-node texture/value cache. Worker→cache fan-out
    /// arrives as `cache_events` passed into `render` by the host;
    /// the renderer applies them at the top of the frame.
    argument_values_cache: ArgumentValuesCache,
}

impl GraphUi {
    pub fn cancel_gesture(&mut self) {
        self.gesture.cancel();
    }

    /// Per-frame entry point. The body splits into three ordered phases:
    ///   1. content — layout, background, connections, nodes
    ///   2. overlays — buttons, details panel, new-node popup
    ///   3. zoom/pan — only when no overlay is hovered
    pub fn render(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &GraphContext<'_>,
        cache_events: Vec<CacheEvent>,
        input: &InputSnapshot,
        output: &mut FrameOutput,
    ) {
        for event in cache_events {
            self.argument_values_cache.apply(event);
        }

        if input.cancel_requested() {
            self.cancel_gesture();
        }

        let rect = self.draw_background_frame(gui);

        gui.scope(StableId::new("graph_ui"))
            .max_rect(rect)
            .clip_rect(rect)
            .show(|gui| {
                let (background_response, pointer_pos) =
                    self.setup_background_interaction(gui, input, rect);

                // Render nodes first so their widgets (body, cache button,
                // remove button, ports) register before we read the
                // background's click state. Otherwise the background —
                // the only widget in scope at this point — would claim
                // clicks that should have gone to nodes, triggering a
                // spurious deselect + cancel_gesture.
                self.render_content(gui, ctx, input, &background_response, pointer_pos, output);

                if background_response.clicked() {
                    self.handle_background_click(ctx, output);
                }

                let overlay_hovered = self.render_overlays(
                    gui,
                    ctx,
                    input,
                    pointer_pos,
                    &background_response,
                    output,
                );

                if !overlay_hovered && (self.gesture.is_idle() || self.gesture.is_panning()) {
                    self.update_zoom_and_pan(
                        gui,
                        input,
                        ctx,
                        &background_response,
                        pointer_pos,
                        output,
                    );
                }
            });
    }

    // ------------------------------------------------------------------------
    // Render phases
    // ------------------------------------------------------------------------

    /// Phase 1 — graph content rendered at the graph's zoom scale.
    fn render_content(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &GraphContext<'_>,
        input: &InputSnapshot,
        background_response: &Response,
        pointer_pos: Option<Pos2>,
        output: &mut FrameOutput,
    ) {
        gui.with_scale(ctx.view_graph.scale, |gui| {
            // Refresh galleys first so every subsequent call site
            // can assume galleys exist for every view-node.
            self.graph_layout.refresh_galleys(gui, ctx);

            // Interact with fresh body rects — layouts are computed on
            // demand from the current gesture offset, so this frame's
            // drag delta accumulates into the gesture before rendering
            // reads it again. No stale-rect flash, no dual pass.
            self.node_ui.handle_node_interactions(
                gui,
                ctx,
                &self.graph_layout,
                output,
                &mut self.gesture,
            );

            self.dots_background.render(gui, ctx);
            self.render_connections(gui, ctx, output);

            // Overlay that swallows background click-through for active
            // gestures. Registered HERE — before ports — so later-registered
            // port widgets keep higher egui z-order and their click/drag
            // responses still fire through. See `maybe_capture_overlay`.
            Self::maybe_capture_overlay(gui, &self.gesture);

            let nodes_result =
                self.node_ui
                    .render_nodes(gui, ctx, &self.graph_layout, output, &self.gesture);

            if let Some(pointer_pos) = pointer_pos {
                self.process_connections(
                    input,
                    ctx,
                    background_response,
                    pointer_pos,
                    nodes_result.port_cmd,
                    &nodes_result.broken_nodes,
                    output,
                );
            }
        });
    }

    /// Phase 2 — overlay UI. Returns `true` if any overlay is hovered,
    /// which suppresses zoom/pan input in phase 3.
    fn render_overlays(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &GraphContext<'_>,
        input: &InputSnapshot,
        pointer_pos: Option<Pos2>,
        background_response: &Response,
        output: &mut FrameOutput,
    ) -> bool {
        let buttons = self.render_buttons(gui, ctx.autorun, output);

        match buttons.action {
            Some(ViewButtonAction::ResetView) => {
                self.emit_zoom_pan(ctx.view_graph, Vec2::ZERO, 1.0, output);
            }
            Some(ViewButtonAction::ViewSelected) => {
                if let Some((scale, pan)) = view_selected_node_target(gui, ctx, &self.graph_layout)
                {
                    self.emit_zoom_pan(ctx.view_graph, pan, scale, output);
                }
            }
            Some(ViewButtonAction::FitAll) => {
                let (scale, pan) = fit_all_nodes_target(gui, ctx, &self.graph_layout);
                self.emit_zoom_pan(ctx.view_graph, pan, scale, output);
            }
            None => {}
        }

        let mut hovered = buttons.response.hovered();
        hovered |= self
            .node_details_ui
            .show(gui, ctx, &mut self.argument_values_cache, output)
            .hovered();
        hovered |=
            self.handle_new_node_popup(gui, input, ctx, pointer_pos, background_response, output);
        hovered
    }

    // ------------------------------------------------------------------------
    // Background
    // ------------------------------------------------------------------------

    fn draw_background_frame(&self, gui: &mut Gui<'_>) -> Rect {
        let rect = gui.rect.shrink(gui.style.big_padding);

        gui.painter().rect(
            rect,
            gui.style.corner_radius,
            gui.style.graph_background.bg_color,
            gui.style.inactive_bg_stroke,
            StrokeKind::Inside,
        );

        rect.shrink(gui.style.corner_radius * 0.5)
    }

    fn setup_background_interaction(
        &self,
        gui: &mut Gui<'_>,
        input: &InputSnapshot,
        rect: Rect,
    ) -> (Response, Option<Pos2>) {
        let graph_bg_id = StableId::new("graph_bg");

        let pointer_pos = input
            .pointer_pos
            .and_then(|pos| rect.contains(pos).then_else(Some(pos), None));

        let response = HitRegion::new(graph_bg_id)
            .rect(rect)
            .sense(Sense::hover() | Sense::drag() | Sense::click())
            .show(gui);

        (response, pointer_pos)
    }
}
