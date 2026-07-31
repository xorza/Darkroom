//! Per-node inspection panels: a floating, read-only summary of a node
//! (identity, inputs, outputs, run status, log) toggled by the `i` chip
//! in the node header.
//!
//! Input lines show the static binding; outputs show only their label.
//! Neither reads a runtime value — a preview node is how you look at one.
//!
//! Panels are **not** palantir `Popup`s — those record into the
//! screen-space `Layer::Popup` and wouldn't track the canvas. Instead
//! [`Inspectors::draw_panels`] records ordinary `Panel`s as direct
//! children of the inner (transformed) canvas in
//! [`crate::gui::canvas::GraphUI::draw`], positioned at the node's world
//! coords — so they pan and scale with the node for free.
//!
//! Each node cycles independently `Closed → Open → Pinned → Closed` on
//! each header-chip click. `Open` (unpinned) panels close on any outside
//! action (clicking a node / bare canvas, panning, zooming); `Pinned`
//! ones persist. State lives on `GraphUI` (not the resettable gesture
//! group) so pinned panels survive tab switches — panels only render for
//! nodes the current `GraphScope` holds, so off-tab ones disappear.

use std::collections::BTreeMap;

use glam::Vec2;
use palantir::{
    Background, Color, Configure, Corners, FontWeight, Panel, Sense, Shadow, Sizing, Spacing,
    Stroke, Text, TextInput, TextStyle, TextWrap, Ui, WidgetId,
};
use scenarium::DataType;
use scenarium::Library;
use scenarium::LogLevel;
use scenarium::NodeId;

use crate::gui::canvas::hits::{CanvasHits, Chip};
use crate::gui::canvas::outer_canvas_widget_id;
use crate::gui::format::fmt_elapsed;
use crate::gui::graph_scope::GraphScope;
use crate::gui::graph_scope::node_scope::NodeScope;
use crate::gui::node::{RecordCtx, exec_color};
use crate::gui::run_state::ExecStatus;
use crate::gui::theme::Theme;
use crate::gui::widgets::support::{colored_text, sized_text};
use scenarium::Binding;

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
///
/// A `BTreeMap`, not a hash map: [`Self::draw_panels`] records straight off
/// this iteration order, so with two panels overlapping, a hashed order would
/// let the one on top change between frames. Node ids are `Ord`, so ordering
/// costs nothing else here — the map is tiny and read once per frame.
#[derive(Default, Debug)]
pub(crate) struct Inspectors {
    modes: BTreeMap<NodeId, InspectMode>,
}

/// Fixed panel width in canvas (pre-transform) units.
const PANEL_WIDTH: f32 = 280.0;
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

    /// Drop transient (`Open`) panels, keeping pinned ones. Called when
    /// an outside action fires and on a tab switch.
    pub(super) fn close_unpinned(&mut self) {
        self.modes.retain(|_, m| *m == InspectMode::Pinned);
    }

    /// Read last-frame chip clicks to cycle node states, close unpinned
    /// panels on an outside action, and drop entries for deleted nodes.
    /// Reads everything off last-frame responses (same timing as the
    /// chip toggle), so a chip click never reads as its own outside
    /// action — the click lands on the chip, not the canvas or a body.
    pub(super) fn apply(&mut self, ui: &Ui, hits: &CanvasHits, graph_scope: GraphScope<'_>) {
        if let Some(node) = hits.chip(Chip::Inspect) {
            match cycle(self.modes.get(&node).copied()) {
                Some(m) => {
                    self.modes.insert(node, m);
                }
                None => {
                    self.modes.remove(&node);
                }
            }
        }
        if outside_action(ui, hits) {
            self.close_unpinned();
        }
        // A panel outlives its node only until the next sweep.
        self.modes.retain(|id, _| graph_scope.contains(*id));
    }

    /// Record a panel for every open inspector, positioned just right of
    /// its node in canvas-world coords. Call inside the inner-canvas
    /// closure, after the node bodies, so panels paint on top and win
    /// hit-tests over the nodes beneath. Walks `modes` in node-id order, so
    /// two overlapping panels keep a stable front-to-back relationship
    /// instead of trading places between frames.
    pub(super) fn draw_panels(&self, ui: &mut Ui, rcx: RecordCtx<'_>) {
        let (theme, graph_scope, geometry) = (rcx.theme, rcx.graph_scope, rcx.geometry);
        for (&id, &mode) in &self.modes {
            let Some(node) = graph_scope.node(id) else {
                continue;
            };
            // The cached body width places the panel just past the node's
            // right edge — cached (not last frame's response) so the
            // anchor holds when the viewport cull skips the node while its
            // panel still peeks into view; absent before the node's first
            // ever record.
            let node_w = geometry
                .node_world_rect(node)
                .map(|r| r.size.w)
                .unwrap_or(theme.node_min_width);
            let pos = node.pos + Vec2::new(node_w + theme.floating_widget_gap, 0.0);
            self.draw_one(ui, rcx, node, mode, pos);
        }
    }

    fn draw_one(
        &self,
        ui: &mut Ui,
        rcx: RecordCtx<'_>,
        node: NodeScope<'_>,
        mode: InspectMode,
        pos: Vec2,
    ) {
        let theme = rcx.theme;
        let logs = rcx.graph_scope.run_state().logs(node.id);
        let error = rcx.graph_scope.run_state().error(node.id);
        // The outline is the *pinned* signal, in the same accent the header's
        // `i` chip uses for its open/pinned states — one color means
        // "inspector held open" on both ends. A transient panel rides on its
        // shadow alone. Width stays constant so pin-cycling never reflows.
        let border = match mode {
            InspectMode::Pinned => theme.colors.badge_graph,
            InspectMode::Open => Color::TRANSPARENT,
        };
        let chrome = Background::rounded(
            theme.colors.node_fill,
            Corners::all(theme.node_corner_radius),
        )
        .with_stroke(Stroke::solid(border, 1.0))
        // Same elevation swatch as the node bodies, so every floating
        // surface casts one kind of shadow (bigger blur — the panel sits higher).
        .with_shadow(Shadow::drop(
            theme.colors.node_ambient_shadow,
            Vec2::new(0.0, 3.0),
            12.0,
        ));
        Panel::vstack()
            .id(inspect_panel_wid(node.id))
            .position(pos)
            .size((Sizing::fixed(PANEL_WIDTH), Sizing::HUG))
            .sense(Sense::CLICK)
            .padding(Spacing::all(8.0))
            .gap(3.0)
            .background(chrome)
            .show(ui, |ui| {
                let title = if node.name().is_empty() {
                    "(unnamed)"
                } else {
                    node.name()
                };
                let repeated_kind = node.kind_label().eq_ignore_ascii_case(title);
                line(ui, title, title_style(ui));
                // The kind line earns its row only when it says something the
                // title doesn't — an unrenamed node repeats its func name.
                if !repeated_kind {
                    line(ui, node.kind_label(), muted_style(theme, ui));
                }

                // Status right under the title — the most-glanceable fact.
                // The colored line is self-labeling; no section header.
                let status_color =
                    exec_color(theme, node.exec_status()).unwrap_or(ui.theme.text.color);
                let status = status_text(ui, node.exec_status());
                line(
                    ui,
                    status,
                    TextStyle {
                        color: status_color,
                        ..body_style(ui)
                    },
                );
                // The actual failure cause beneath the bare "errored" line, in
                // the error color — this is what turns a generic status into
                // an actionable message (e.g. "no light frames provided").
                if let Some(message) = error {
                    line(
                        ui,
                        message,
                        TextStyle {
                            color: theme.colors.exec_errored_glow,
                            ..body_style(ui)
                        },
                    );
                }
                if node.sink() {
                    line(ui, "sink", muted_style(theme, ui));
                }
                if !node.description().is_empty() {
                    line(ui, node.description(), muted_style(theme, ui));
                }

                let library = rcx.graph_scope.library();
                if node.inputs().len() > 0 {
                    section(ui, theme, "Inputs");
                    for input in node.inputs() {
                        let val = value_str(ui, input.binding());
                        port_row(ui, theme, library, input.name(), input.ty(), Some(val));
                    }
                }

                if node.outputs().len() > 0 {
                    section(ui, theme, "Outputs");
                    for output in node.outputs() {
                        port_row(ui, theme, library, output.name(), &output.ty(), None);
                    }
                }

                // Log: this node's lines from the last run, level-colored.
                // Capped to the most recent few so the panel can't grow
                // unbounded.
                if !logs.is_empty() {
                    section(ui, theme, "Log");
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
fn outside_action(ui: &Ui, hits: &CanvasHits) -> bool {
    let oc = ui.response_for(outer_canvas_widget_id());
    // Any-button drag: left rubber-bands, middle pans, right scribbles
    // the breaker — all of them count as "acted on the canvas".
    let dragged = oc.left.drag.dragging() || oc.middle.drag.dragging() || oc.right.drag.dragging();
    let canvas_acted = oc.left.clicked()
        || dragged
        || oc.scroll.lines != Vec2::ZERO
        || oc.scroll.pixels != Vec2::ZERO
        || (oc.scroll.zoom - 1.0).abs() > f32::EPSILON;
    // The node half comes off this frame's sweep.
    canvas_acted || hits.body_acted().is_some()
}

fn line<'a>(ui: &mut Ui, text: impl Into<TextInput<'a>>, style: TextStyle) {
    Text::new(text)
        .style(&style)
        .text_wrap(TextWrap::Wrap)
        .show(ui);
}

/// A bold section label with breathing room above it, so the groups read
/// by proximity inside the panel's tight line rhythm.
fn section(ui: &mut Ui, theme: &Theme, text: &'static str) {
    Text::new(text)
        .style(&TextStyle {
            weight: FontWeight::Bold,
            ..muted_style(theme, ui)
        })
        .margin(Spacing::new(0.0, 8.0, 0.0, 0.0))
        .show(ui);
}

/// One port row, two tiers: a muted label line (the port name, plus its
/// type when the two differ), then the value on its own full-strength
/// line — so a long value wraps cleanly instead of orphaning its tail
/// tokens mid-`name: type = value` string. `None` value renders the label
/// alone (an output that hasn't run).
fn port_row(
    ui: &mut Ui,
    theme: &Theme,
    library: &Library,
    name: &str,
    ty: &DataType,
    val: Option<TextInput<'static>>,
) {
    let label = port_label(library, name, ty);
    line(ui, label, muted_style(theme, ui));
    if let Some(v) = val {
        line(ui, v, body_style(ui));
    }
}

/// The label tier of a port row. Skips the type when it repeats the port
/// name (`Image · Image`, the dominant case; `Path · path` likewise) —
/// otherwise `name · type`, so even a valueless port announces its type.
///
/// The dominant case hands back the declaration's own `&str`, copied into
/// the text arena only when the row is actually shown — so the common port
/// row costs no string at all. Only the differing case builds one.
fn port_label<'a>(library: &Library, name: &'a str, ty: &DataType) -> TextInput<'a> {
    let ty_name = library.type_name(ty);
    if ty_name.eq_ignore_ascii_case(name) {
        return TextInput::Borrowed(name);
    }
    TextInput::Owned(format!("{name} \u{b7} {ty_name}"))
}

fn title_style(ui: &Ui) -> TextStyle {
    sized_text(ui, 14.0)
}

/// The panel's de-emphasized ink. `port_label`, not `text_muted`: the panel
/// surface is the node fill, exactly the combination the light palette's
/// `text_muted` is too faint for (see the `port_label` slot).
fn muted_style(theme: &Theme, ui: &Ui) -> TextStyle {
    colored_text(ui, theme.colors.port_label, 11.0)
}

fn body_style(ui: &Ui) -> TextStyle {
    sized_text(ui, 12.0)
}

/// The value tier of an input row.
///
/// The two fixed answers stay borrowed and the formatted one goes
/// straight into the text arena, so an open panel builds no owned string
/// per input per frame.
fn value_str(ui: &mut Ui, binding: Option<&Binding>) -> TextInput<'static> {
    match binding {
        None => TextInput::Borrowed("—"),
        Some(Binding::Bind(_)) => TextInput::Borrowed("linked"),
        // `StaticValue::Display` — the same formatter a preview node's
        // value goes through via `DynamicValue::Display`, so a static
        // binding and a live one can't render one float two ways.
        Some(Binding::Const(v)) => TextInput::Interned(ui.fmt(format_args!("{v}"))),
    }
}

fn status_text(ui: &mut Ui, status: ExecStatus) -> TextInput<'static> {
    match status {
        ExecStatus::None => TextInput::Borrowed("not run"),
        ExecStatus::Cached => TextInput::Borrowed("cached"),
        ExecStatus::Executed(secs) => {
            TextInput::Interned(ui.fmt(format_args!("ran in {}", fmt_elapsed(secs))))
        }
        ExecStatus::Running(at) => TextInput::Interned(ui.fmt(format_args!(
            "running… {}",
            fmt_elapsed(at.elapsed().as_secs_f64())
        ))),
        ExecStatus::MissingInputs => TextInput::Borrowed("missing inputs"),
        ExecStatus::Errored => TextInput::Borrowed("errored"),
    }
}

/// Stable id for a node's inspector toggle chip in the header.
pub(crate) fn inspect_badge_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.inspect_badge", node_id))
}

/// Stable id for a node's floating inspection panel.
pub(super) fn inspect_panel_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.inspect_panel", node_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rendered text of a label, whichever form it came back in.
    fn shown(input: &TextInput<'_>) -> String {
        match input {
            TextInput::Borrowed(text) => (*text).to_owned(),
            TextInput::Owned(text) => text.clone(),
            TextInput::Interned(text) => text.borrow_str().to_owned(),
        }
    }

    #[test]
    fn port_label_dedups_the_type_and_reuses_the_scenes_own_handle() {
        use scenarium::FsPathConfig;
        use std::sync::Arc;

        let lib = Library::default();
        let path_ty = DataType::FsPath(Arc::new(FsPathConfig::default()));

        // Type repeats the name (case-insensitively) → the name alone, no
        // "Image · Image" stutter; distinct type → "name · type". A path
        // port still announces its type when the name differs — the old
        // formatter dropped it entirely for valueless path ports.
        let cases: [(&str, &DataType, &str); 4] = [
            ("Float", &DataType::Float, "Float"),
            ("Brightness", &DataType::Float, "Brightness \u{b7} float"),
            ("Path", &path_ty, "Path"),
            ("Output", &path_ty, "Output \u{b7} path"),
        ];
        for (name, ty, expected) in cases {
            let label = port_label(&lib, name, ty);
            assert_eq!(shown(&label), expected, "label for {name}");
            // The deduped case is the dominant one, and it must hand back the
            // declaration's own `&str` rather than build a string: that is the
            // whole reason an open panel costs no allocation per port row per
            // frame. Only the differing case owns anything.
            let deduped = expected == name;
            assert_eq!(
                matches!(label, TextInput::Borrowed(_)),
                deduped,
                "{name}: a deduped label borrows the declared name, a \
                 combined one owns its string",
            );
        }
    }

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
