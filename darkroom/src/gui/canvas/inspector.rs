//! Per-node inspection panels: a floating, read-only summary of a node
//! (identity, inputs, outputs, run status, log) toggled by the `i` chip
//! in the node header.
//!
//! Input/output lines show the last run's computed runtime value when one
//! is available (pulled on demand into [`crate::gui::run_state::RunState`] for
//! open panels), falling back to the static binding otherwise; image
//! ports also render a preview thumbnail beneath their line.
//!
//! Panels are **not** aperture `Popup`s — those record into the
//! screen-space `Layer::Popup` and wouldn't track the canvas. Instead
//! [`Inspectors::draw_panels`] records ordinary `Panel`s as direct
//! children of the inner (transformed) canvas in
//! [`crate::gui::canvas::GraphUI::frame`], positioned at the node's world
//! coords — so they pan and scale with the node for free.
//!
//! Each node cycles independently `Closed → Open → Pinned → Closed` on
//! each header-chip click. `Open` (unpinned) panels close on any outside
//! action (clicking a node / bare canvas, panning, zooming); `Pinned`
//! ones persist. State lives on `GraphUI` (not the resettable gesture
//! group) so pinned panels survive tab switches — panels only render for
//! nodes present in the current `Scene`, so off-tab ones disappear.

use std::collections::HashMap;

use aperture::{
    Background, Color, Configure, Corners, ImageFit, Panel, Sense, Shadow, Shape, Sizing, Spacing,
    Stroke, Text, TextStyle, TextWrap, Ui, WidgetId,
};
use glam::Vec2;
use scenarium::data::{DataType, StaticValue};
use scenarium::execution::stats::LogLevel;
use scenarium::graph::NodeId;
use scenarium::library::Library;

use crate::gui::canvas::outer_canvas_widget_id;
use crate::gui::node::header::fmt_elapsed;
use crate::gui::node::{exec_color, node_widget_id};
use crate::gui::node_values::PortValueView;
use crate::gui::run_state::{ExecStatus, RunState};
use crate::gui::scene::{InputBindingView, Scene, SceneNode};
use crate::gui::theme::Theme;

/// Open state of a single node's inspector. Absence from
/// [`Inspectors::modes`] is the third, `Closed`, state.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum InspectMode {
    /// Transient: closes on the next outside action.
    Open,
    /// Sticky: stays open through outside actions; chip renders checked.
    Pinned,
}

/// Open inspection panels, keyed by node. Survives tab switches; panels
/// only paint for nodes in the current scene.
#[derive(Default, Debug)]
pub(crate) struct Inspectors {
    modes: HashMap<NodeId, InspectMode>,
}

/// Cross-cutting refs every inspector panel reads, bundled so `draw_one`
/// takes a context rather than a fistful of loose arguments.
#[derive(Debug)]
struct PanelDraw<'a> {
    theme: &'a Theme,
    library: &'a Library,
    scene: &'a Scene,
    run_state: &'a RunState,
}

/// Fixed panel width in canvas (pre-transform) units.
const PANEL_WIDTH: f32 = 210.0;
/// Max width of an inline preview thumbnail (panel width minus the
/// panel's 8 px padding on each side).
const PREVIEW_MAX_WIDTH: f32 = PANEL_WIDTH - 16.0;
/// Gap between the node's right edge and the panel's left edge.
const PANEL_GAP: f32 = 16.0;
/// Most recent log lines shown per node, so the panel stays bounded.
const LOG_LINE_CAP: usize = 20;

/// Next state in the `Closed → Open → Pinned → Closed` cycle. `None`
/// is the `Closed` state.
fn cycle(mode: Option<InspectMode>) -> Option<InspectMode> {
    match mode {
        None => Some(InspectMode::Open),
        Some(InspectMode::Open) => Some(InspectMode::Pinned),
        Some(InspectMode::Pinned) => None,
    }
}

impl Inspectors {
    /// The mode for a node, for the header chip to style itself.
    pub(crate) fn mode(&self, id: NodeId) -> Option<InspectMode> {
        self.modes.get(&id).copied()
    }

    /// Nodes with an open panel (either mode), for the frame loop to fetch
    /// runtime values for.
    pub(crate) fn open_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.modes.keys().copied()
    }

    /// Drop transient (`Open`) panels, keeping pinned ones. Called when
    /// an outside action fires and on a tab switch.
    pub(crate) fn close_unpinned(&mut self) {
        self.modes.retain(|_, m| *m == InspectMode::Pinned);
    }

    /// Read last-frame chip clicks to cycle node states, close unpinned
    /// panels on an outside action, and drop entries for deleted nodes.
    /// Reads everything off last-frame responses (same timing as the
    /// chip toggle), so a chip click never reads as its own outside
    /// action — the click lands on the chip, not the canvas or a body.
    pub(crate) fn apply(&mut self, ui: &Ui, scene: &Scene) {
        for n in &scene.nodes {
            if ui.response_for(inspect_badge_wid(n.id)).clicked {
                match cycle(self.modes.get(&n.id).copied()) {
                    Some(m) => {
                        self.modes.insert(n.id, m);
                    }
                    None => {
                        self.modes.remove(&n.id);
                    }
                }
            }
        }
        if outside_action(ui, scene) {
            self.close_unpinned();
        }
        self.modes
            .retain(|id, _| scene.nodes.iter().any(|n| n.id == *id));
    }

    /// Record a panel for every open inspector, positioned just right of
    /// its node in canvas-world coords. Call inside the inner-canvas
    /// closure, after the node bodies, so panels paint on top and win
    /// hit-tests over the nodes beneath.
    pub(crate) fn draw_panels(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        library: &Library,
        scene: &Scene,
        run_state: &RunState,
    ) {
        let ctx = PanelDraw {
            theme,
            library,
            scene,
            run_state,
        };
        for (&id, &mode) in &self.modes {
            let Some(node) = scene.nodes.iter().find(|n| n.id == id) else {
                continue;
            };
            // Boundary nodes (SubgraphInput/SubgraphOutput) are pure
            // routing — no runtime values or status — and the chip is
            // suppressed on the header; skip any stale pinned entry too.
            if node.boundary {
                continue;
            }
            // Last frame's body width places the panel just past the
            // node's right edge; absent on the node's first frame.
            let node_w = ui
                .response_for(node_widget_id(id))
                .layout_rect
                .map(|r| r.size.w)
                .unwrap_or(theme.node_min_width);
            let pos = node.pos + Vec2::new(node_w + PANEL_GAP, 0.0);
            self.draw_one(ui, &ctx, node, mode, pos);
        }
    }

    fn draw_one(
        &self,
        ui: &mut Ui,
        ctx: &PanelDraw,
        node: &SceneNode,
        mode: InspectMode,
        pos: Vec2,
    ) {
        let theme = ctx.theme;
        let scene = ctx.scene;
        let logs = ctx.run_state.logs(node.id);
        let errors = ctx.run_state.errors(node.id);
        let values = ctx.run_state.values(node.id);
        let border = match mode {
            InspectMode::Pinned => theme.colors.text_muted,
            InspectMode::Open => theme.colors.node_border,
        };
        let chrome = Background {
            fill: theme.colors.node_fill.into(),
            stroke: Stroke::solid(border, 1.0),
            corners: Corners::all(theme.node_corner_radius),
            shadow: Shadow {
                color: Color::linear_rgb(0.0, 0.0, 0.0).with_alpha(0.45),
                offset: Vec2::new(0.0, 3.0),
                blur: 12.0,
                spread: 0.0,
                inset: false,
            },
        };
        Panel::vstack()
            .id(inspect_panel_wid(node.id))
            .position(pos)
            .size((Sizing::Fixed(PANEL_WIDTH), Sizing::Hug))
            .sense(Sense::CLICK)
            .padding(Spacing::all(8.0))
            .gap(3.0)
            .background(chrome)
            .show(ui, |ui| {
                let title = if node.name.is_empty() {
                    "(unnamed)"
                } else {
                    node.name.as_str()
                };
                line(ui, title, title_style(ui));
                line(ui, node.kind_label.as_str(), muted_style(ui));
                let description = node.description.as_str();
                if !description.is_empty() {
                    line(ui, description, muted_style(ui));
                }

                let inputs = scene.inputs(node.inputs);
                if !inputs.is_empty() {
                    line(ui, "Inputs", section_style(ui));
                    for (i, input) in inputs.iter().enumerate() {
                        let name = input.name.as_str();
                        let ty = Some(&input.ty);
                        // Runtime value when this run computed one; else
                        // fall back to the static binding.
                        match values.and_then(|v| v.inputs.get(i)) {
                            Some(pv) => {
                                line(
                                    ui,
                                    &port_line(ctx.library, name, ty, Some(&pv.text)),
                                    body_style(ui),
                                );
                                draw_preview(ui, node.id, "in", i, pv);
                            }
                            None => {
                                let val = value_str(&input.binding);
                                line(
                                    ui,
                                    &port_line(ctx.library, name, ty, Some(val.as_str())),
                                    body_style(ui),
                                );
                            }
                        }
                    }
                }

                let outputs = scene.outputs(node.outputs);
                if !outputs.is_empty() {
                    line(ui, "Outputs", section_style(ui));
                    for (i, output) in outputs.iter().enumerate() {
                        let name = output.name.as_str();
                        let ty = Some(&output.ty);
                        match values.and_then(|v| v.outputs.get(i)) {
                            Some(pv) => {
                                line(
                                    ui,
                                    &port_line(ctx.library, name, ty, Some(&pv.text)),
                                    body_style(ui),
                                );
                                draw_preview(ui, node.id, "out", i, pv);
                            }
                            None => {
                                line(ui, &port_line(ctx.library, name, ty, None), body_style(ui))
                            }
                        }
                    }
                }

                line(ui, "Status", section_style(ui));
                let status_color =
                    exec_color(theme, node.exec_status).unwrap_or(ui.theme.text.color);
                line(
                    ui,
                    &status_text(node.exec_status),
                    TextStyle {
                        color: status_color,
                        ..body_style(ui)
                    },
                );
                // The actual failure cause(s) beneath the bare "errored" line,
                // in the error color — this is what turns a generic status into
                // an actionable message (e.g. "no light frames provided").
                for message in errors {
                    line(
                        ui,
                        message,
                        TextStyle {
                            color: theme.colors.exec_errored_glow,
                            ..body_style(ui)
                        },
                    );
                }
                if let Some(flags) = flag_text(node) {
                    line(ui, &flags, muted_style(ui));
                }

                // Log: this node's lines from the last run, level-colored.
                // Capped to the most recent few so the panel can't grow
                // unbounded.
                if !logs.is_empty() {
                    line(ui, "Log", section_style(ui));
                    let start = logs.len().saturating_sub(LOG_LINE_CAP);
                    for entry in &logs[start..] {
                        line(
                            ui,
                            &entry.message,
                            TextStyle {
                                color: log_color(theme, ui, entry.level),
                                ..body_style(ui)
                            },
                        );
                    }
                }
            });
    }
}

/// Color for a log line by level: info reads as muted body text, warn
/// reuses the missing-inputs glow (orange), error the errored glow (red).
fn log_color(theme: &Theme, ui: &Ui, level: LogLevel) -> Color {
    match level {
        LogLevel::Info => ui.theme.text.color.with_alpha(0.85),
        LogLevel::Warn => theme.colors.exec_missing_glow,
        LogLevel::Error => theme.colors.exec_errored_glow,
    }
}

/// Did the user act outside any inspection panel this frame? Clicking a
/// node body, clicking bare canvas, or panning/zooming the canvas all
/// count; clicks inside a panel or on a chip don't (those widgets
/// capture the press, so neither the canvas nor a body sees it).
fn outside_action(ui: &Ui, scene: &Scene) -> bool {
    let oc = ui.response_for(outer_canvas_widget_id());
    let canvas_acted = oc.clicked
        || oc.drag_delta().is_some()
        || oc.scroll_lines != Vec2::ZERO
        || oc.scroll_pixels != Vec2::ZERO
        || (oc.zoom_factor - 1.0).abs() > f32::EPSILON;
    let node_acted = scene.nodes.iter().any(|n| {
        let r = ui.response_for(node_widget_id(n.id));
        r.clicked || r.drag_started()
    });
    canvas_acted || node_acted
}

fn line(ui: &mut Ui, text: &str, style: TextStyle) {
    Text::new(text.to_owned())
        .style(style)
        .text_wrap(TextWrap::Wrap)
        .show(ui);
}

/// Draw a port's preview thumbnail beneath its value line: a fixed-size
/// panel (aspect-preserving, capped at [`PREVIEW_MAX_WIDTH`]) painting the
/// registered texture. No-op when the port has no preview.
fn draw_preview(ui: &mut Ui, node_id: NodeId, side: &str, idx: usize, pv: &PortValueView) {
    let Some(handle) = &pv.preview else {
        return;
    };
    let size = handle.size();
    if size.x == 0 || size.y == 0 {
        return;
    }
    let aspect = size.x as f32 / size.y as f32;
    let w = PREVIEW_MAX_WIDTH.min(size.x as f32);
    let h = w / aspect;
    Panel::vstack()
        .id(WidgetId::from_hash((
            "inspector.preview",
            node_id,
            side,
            idx,
        )))
        .size((Sizing::Fixed(w), Sizing::Fixed(h)))
        .show(ui, |ui| {
            ui.add_shape(Shape::Image {
                handle: handle.clone(),
                local_rect: None,
                fit: ImageFit::Contain,
                tint: Color::WHITE,
            });
        });
}

fn title_style(ui: &Ui) -> TextStyle {
    TextStyle {
        font_size_px: 14.0,
        ..ui.theme.text
    }
}

fn section_style(ui: &Ui) -> TextStyle {
    TextStyle {
        color: ui.theme.text.color.with_alpha(0.7),
        font_size_px: 11.0,
        ..ui.theme.text
    }
}

fn muted_style(ui: &Ui) -> TextStyle {
    TextStyle {
        color: ui.theme.text.color.with_alpha(0.6),
        font_size_px: 11.0,
        ..ui.theme.text
    }
}

fn body_style(ui: &Ui) -> TextStyle {
    TextStyle {
        font_size_px: 12.0,
        ..ui.theme.text
    }
}

/// Format one port row. Path-typed ports show `name: value` (the value
/// already conveys "path"); other ports show `name: type = value`, or
/// `name: type` when no value is available.
fn port_line(library: &Library, name: &str, ty: Option<&DataType>, val: Option<&str>) -> String {
    if matches!(ty, Some(DataType::FsPath(_))) {
        match val {
            Some(v) => format!("{name}: {v}"),
            None => format!("{name}: path"),
        }
    } else {
        let ty = ty
            .map(|t| library.type_name(t).into_owned())
            .unwrap_or_default();
        match val {
            Some(v) => format!("{name}: {ty} = {v}"),
            None => format!("{name}: {ty}"),
        }
    }
}

fn value_str(b: &InputBindingView) -> String {
    match b {
        InputBindingView::None => "—".to_owned(),
        InputBindingView::Bind => "linked".to_owned(),
        InputBindingView::Const(v) => static_value_str(v),
    }
}

fn static_value_str(v: &StaticValue) -> String {
    match v {
        StaticValue::Null => "null".to_owned(),
        StaticValue::Float(f) => format!("{f}"),
        StaticValue::Int(i) => format!("{i}"),
        StaticValue::Bool(b) => format!("{b}"),
        StaticValue::String(s) => format!("\"{s}\""),
        StaticValue::FsPath(path) => path.clone(),
        StaticValue::Enum(variant) => variant.clone(),
    }
}

fn status_text(status: ExecStatus) -> String {
    match status {
        ExecStatus::None => "not run".to_owned(),
        ExecStatus::Cached => "cached".to_owned(),
        ExecStatus::Executed(secs) => format!("ran in {}", fmt_elapsed(secs)),
        ExecStatus::Running(at) => format!("running… {}", fmt_elapsed(at.elapsed().as_secs_f64())),
        ExecStatus::MissingInputs => "missing inputs".to_owned(),
        ExecStatus::Errored => "errored".to_owned(),
    }
}

/// `terminal` flag for the footer, or `None` when the node isn't a terminal.
fn flag_text(node: &SceneNode) -> Option<String> {
    node.terminal.then(|| "terminal".to_owned())
}

/// Stable id for a node's inspector toggle chip in the header.
pub(crate) fn inspect_badge_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.inspect_badge", node_id))
}

/// Stable id for a node's floating inspection panel.
fn inspect_panel_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.inspect_panel", node_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_walks_closed_open_pinned_closed() {
        // Closed (None) → Open → Pinned → Closed, matching the chip's
        // three-click loop.
        assert_eq!(cycle(None), Some(InspectMode::Open));
        assert_eq!(cycle(Some(InspectMode::Open)), Some(InspectMode::Pinned));
        assert_eq!(cycle(Some(InspectMode::Pinned)), None);
    }

    #[test]
    fn close_unpinned_drops_open_keeps_pinned() {
        let open = NodeId::unique();
        let pinned = NodeId::unique();
        let mut ins = Inspectors::default();
        ins.modes.insert(open, InspectMode::Open);
        ins.modes.insert(pinned, InspectMode::Pinned);

        ins.close_unpinned();

        assert_eq!(ins.mode(open), None, "transient panel closed");
        assert_eq!(
            ins.mode(pinned),
            Some(InspectMode::Pinned),
            "pinned panel survives the outside action"
        );
    }
}
