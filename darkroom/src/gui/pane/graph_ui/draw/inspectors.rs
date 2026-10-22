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
//! [`crate::gui::pane::graph_ui::GraphUi::draw`], positioned at the node's world
//! coords — so they pan and scale with the node for free.
//!
//! Each node cycles independently `Closed → Open → Pinned → Closed` on
//! each header-chip click. `Open` (unpinned) panels close on any outside
//! action (clicking a node / bare canvas, panning, zooming); `Pinned`
//! ones persist. State lives on `GraphUi` (not the resettable gesture
//! group) so pinned panels survive tab switches — panels only render for
//! nodes the current `GraphCtx` holds, so off-tab ones disappear.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use palantir::prelude::*;
use palantir::{FontWeight, TextInput, TextWrap, ZoomFactor};
use scenarium::DataType;
use scenarium::Library;
use scenarium::LogLevel;
use scenarium::NodeId;

use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::draw_ctx::DrawCtx;
use crate::gui::pane::graph_ui::node_ui::NodeDrawOutcome;
use crate::gui::pane::graph_ui::node_ui::node_widget;
use crate::gui::pane::graph_ui::node_ui::wid;
use crate::gui::state::run_state::ExecStatus;
use crate::gui::theme::Theme;
use crate::gui::widgets::format::elapsed::Elapsed;
use crate::gui::widgets::support;
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

/// The panel's shadow blur: a node body's is the status glow's, and the panel
/// floats above the bodies, so it casts the softer shadow of a higher surface.
const PANEL_SHADOW_BLUR: f32 = 12.0;

/// Fixed panel width in canvas (pre-transform) units.
const PANEL_WIDTH: f32 = 280.0;
/// Most recent log lines shown per node, so the panel stays bounded.
const LOG_LINE_CAP: usize = 20;

/// Next state in the `Closed → Open → Pinned → Closed` cycle. `None`
/// is the `Closed` state.
const fn cycle(mode: Option<InspectMode>) -> Option<InspectMode> {
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

    /// Drop transient (`Open`) panels, keeping pinned ones. Called when an
    /// outside action fires.
    pub(crate) fn close_unpinned(&mut self) {
        self.modes.retain(|_, m| *m == InspectMode::Pinned);
    }

    /// Cycle the node whose `i` chip was clicked, and close the unpinned
    /// panels on an outside action.
    ///
    /// Both node-side facts come from the record pass that just drew them
    /// ([`NodeDrawOutcome`]);
    /// the canvas half is read here. Everything is last-frame responses either
    /// way, so a chip click never reads as its own outside action — the click
    /// lands on the chip, not on the canvas or a body.
    ///
    /// Dropping entries for deleted nodes is *not* here: that needs only the
    /// document, so it runs once a frame with the canvas's other
    /// `NodeId`-keyed sweeps ([`Self::retain_nodes`]) rather than per record
    /// pass with the input handling.
    pub(crate) fn apply(&mut self, ui: &Ui, outcome: &NodeDrawOutcome) {
        if let Some(node) = outcome.inspect_toggled {
            match cycle(self.modes.get(&node).copied()) {
                Some(m) => {
                    self.modes.insert(node, m);
                }
                None => {
                    self.modes.remove(&node);
                }
            }
        }
        if outcome.body_acted || canvas_acted(ui) {
            self.close_unpinned();
        }
    }

    /// Drop panels whose node the document has stopped holding.
    ///
    /// Pure ballast: [`Self::draw_panels`] already skips an entry whose node
    /// does not resolve, so a mode that outlives its node costs memory and
    /// nothing else — which is what lets this run once a frame beside the
    /// canvas's other sweeps rather than inside the record pass.
    ///
    /// Asked of the document rather than of the scene: `modes` deliberately
    /// survives a tab switch, so only the node being *gone* may drop an entry.
    pub(crate) fn retain_nodes(&mut self, keep: impl Fn(NodeId) -> bool) {
        self.modes.retain(|id, _| keep(*id));
    }

    /// Record a panel for every open inspector, positioned just right of
    /// its node in canvas-world coords. Call inside the inner-canvas
    /// closure, after the node bodies, so panels paint on top and win
    /// hit-tests over the nodes beneath. Walks `modes` in node-id order, so
    /// two overlapping panels keep a stable front-to-back relationship
    /// instead of trading places between frames.
    pub(crate) fn draw_panels(&self, ui: &mut Ui, draw_ctx: DrawCtx<'_>) {
        let (theme, graph_ctx, geometry) =
            (draw_ctx.theme(), draw_ctx.graph_ctx(), draw_ctx.geometry());
        for (&id, &mode) in &self.modes {
            let Some(node) = graph_ctx.node(id) else {
                continue;
            };
            // The cached body width places the panel just past the node's
            // right edge — cached (not last frame's response) so the
            // anchor holds when the viewport cull skips the node while its
            // panel still peeks into view; absent before the node's first
            // ever record.
            let node_w = geometry
                .node_world_rect(node)
                .map_or(theme.card.min_width, |r| r.size.w);
            let pos = node.pos + Vec2::new(node_w + theme.floating_widget_gap, 0.0);
            let node_ctx = node.with_hover(wid::hover(ui, node.id));
            InspectPanel {
                node: node_ctx,
                mode,
                pos,
            }
            .show(ui);
        }
    }
}

/// One open inspection panel: its node, how it is held open, and where it
/// sits in canvas-world coords.
#[derive(Debug, Clone, Copy)]
struct InspectPanel<'a> {
    node: NodeCtx<'a>,
    mode: InspectMode,
    pos: Vec2,
}

impl InspectPanel<'_> {
    fn show(self, ui: &mut Ui) {
        let Self {
            node: node_ctx,
            mode,
            pos,
        } = self;
        let (theme, node) = (node_ctx.theme(), node_ctx);
        let logs = node_ctx.graph_ctx.run_state().logs(node.id);
        let error = match node.exec_status() {
            ExecStatus::Errored(message) => message.as_deref(),
            _ => None,
        };
        // The outline is the *pinned* signal, in the same accent the header's
        // `i` chip uses for its open/pinned states — one color means
        // "inspector held open" on both ends. A transient panel rides on its
        // shadow alone. Width stays constant so pin-cycling never reflows.
        let border = match mode {
            InspectMode::Pinned => theme.colors.badge_graph,
            InspectMode::Open => RgbaF32::TRANSPARENT,
        };
        let chrome = Chrome::rounded(theme.card.fill, Corners::all(theme.card.corner_radius))
            .with_border(Stroke::new(border, theme.card.border_width))
            // Same elevation swatch as the node bodies, so every floating
            // surface casts one kind of shadow (bigger blur — the panel sits higher).
            .with_shadow(theme.card.elevation_shadow(PANEL_SHADOW_BLUR));
        Panel::vstack()
            .id(inspect_panel_wid(node.id))
            .pos(pos)
            .size((Sizing::fixed(PANEL_WIDTH), Sizing::HUG))
            .sense(Sense::CLICK)
            .padding(Spacing::all(8.0))
            .gap(3.0)
            .chrome(chrome)
            .show(ui, |ui| {
                let title = if node.name().is_empty() {
                    "(unnamed)"
                } else {
                    node.name()
                };
                let repeated_kind = node.kind_label().eq_ignore_ascii_case(title);
                line(ui, title, title_style(theme, ui));
                // The kind line earns its row only when it says something the
                // title doesn't — an unrenamed node repeats its func name.
                if !repeated_kind {
                    line(ui, node.kind_label(), muted_style(theme, ui));
                }

                // Status right under the title — the most-glanceable fact.
                // The colored line is self-labeling; no section header.
                let status_color = node_widget::exec_color(theme, node.exec_status())
                    .unwrap_or(ui.theme().text.color);
                let status = status_text(ui, node.exec_status());
                line(
                    ui,
                    status,
                    TextStyle {
                        color: status_color,
                        ..body_style(theme, ui)
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
                            color: theme.status.error,
                            ..body_style(theme, ui)
                        },
                    );
                }
                if node.sink() {
                    line(ui, "sink", muted_style(theme, ui));
                }
                if !node.description().is_empty() {
                    line(ui, node.description(), muted_style(theme, ui));
                }

                let library = node_ctx.graph_ctx.library();
                if node.inputs().len() > 0 {
                    section(ui, theme, "Inputs");
                    for input in node.inputs() {
                        let val = value_str(ui, input.binding());
                        InspectedPort {
                            label: PortLabel::resolve(library, input.name(), input.data_type()),
                            value: Some(val),
                        }
                        .show(ui, theme);
                    }
                }

                if node.outputs().len() > 0 {
                    section(ui, theme, "Outputs");
                    for output in node.outputs() {
                        InspectedPort {
                            label: PortLabel::resolve(library, output.name(), output.data_type()),
                            value: None,
                        }
                        .show(ui, theme);
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
                                ..body_style(theme, ui)
                            },
                        );
                    }
                }
            });
    }
}

/// `RgbaF32` for a log line by level: info reads as muted body text, warn
/// reuses the missing-inputs glow (orange), error the errored glow (red).
fn log_color(theme: &Theme, ui: &Ui, level: LogLevel) -> RgbaF32 {
    match level {
        LogLevel::Info => ui.theme().text.color,
        LogLevel::Warn => theme.status.warning,
        LogLevel::Error => theme.status.error,
    }
}

/// Did the user act on the bare canvas this frame — the canvas half of an
/// action outside every inspection panel, beside a node body acted on?
/// Clicking or panning/zooming the canvas counts; clicks inside a panel or on
/// a chip don't (those widgets capture the press, so the canvas never sees
/// it).
fn canvas_acted(ui: &Ui) -> bool {
    let oc = ui.response_for(canvas::outer_wid());
    // Any-button drag: left rubber-bands, middle pans, right scribbles
    // the breaker — all of them count as "acted on the canvas".
    let dragged = oc.left.drag.is_live() || oc.middle.drag.is_live() || oc.right.drag.is_live();
    oc.left.clicked()
        || dragged
        || oc.scroll.lines != Vec2::ZERO
        || oc.scroll.pixels != Vec2::ZERO
        || oc.scroll.zoom != ZoomFactor::ONE
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
            weight: FontWeight::BOLD,
            ..muted_style(theme, ui)
        })
        .margin(Spacing::new(0.0, 8.0, 0.0, 0.0))
        .show(ui);
}

/// One port row, two tiers: a muted label line (the port name, plus its
/// type when the two differ), then the value on its own full-strength
/// line — so a long value wraps cleanly instead of orphaning its tail
/// tokens mid-`name: type = value` string. A `None` value renders the label
/// alone (an output that hasn't run).
#[derive(Debug)]
struct InspectedPort<'a> {
    label: PortLabel<'a>,
    value: Option<TextInput<'static>>,
}

impl InspectedPort<'_> {
    fn show(self, ui: &mut Ui, theme: &Theme) {
        let text = fmt!(ui, "{}", self.label);
        line(ui, text, muted_style(theme, ui));
        if let Some(v) = self.value {
            line(ui, v, body_style(theme, ui));
        }
    }
}

/// The label tier of a port row. Skips the type when it repeats the port
/// name (`Image · Image`, the dominant case; `Path · path` likewise) —
/// otherwise `name · type`, so even a valueless port announces its type.
///
/// Renders on demand into the record pass's text arena, which is where the
/// name was headed anyway: an open panel builds no owned string per port per
/// frame, in either case.
#[derive(Debug)]
struct PortLabel<'a> {
    name: &'a str,
    /// The declared type's display name, or `None` when it merely repeats
    /// `name` and the row would read as a stutter.
    type_name: Option<Cow<'a, str>>,
}

impl<'a> PortLabel<'a> {
    fn resolve(library: &'a Library, name: &'a str, data_type: &DataType) -> Self {
        let type_name = library.type_name(data_type);
        let type_name = (!type_name.eq_ignore_ascii_case(name)).then_some(type_name);
        Self { name, type_name }
    }
}

impl Display for PortLabel<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.name)?;
        match &self.type_name {
            Some(type_name) => write!(f, " · {type_name}"),
            None => Ok(()),
        }
    }
}

fn title_style(theme: &Theme, ui: &Ui) -> TextStyle {
    support::sized_text(ui, theme.text.title)
}

/// The panel's de-emphasized ink. `port_label`, not `text_muted`: the panel
/// surface is the node fill, exactly the combination the light palette's
/// `text_muted` is too faint for (see the `port_label` slot).
fn muted_style(theme: &Theme, ui: &Ui) -> TextStyle {
    support::colored_text(ui, theme.ports.label, theme.text.label)
}

fn body_style(theme: &Theme, ui: &Ui) -> TextStyle {
    support::sized_text(ui, theme.text.body)
}

/// The value tier of an input row.
///
/// The two fixed answers stay borrowed and the formatted one goes
/// straight into the text arena, so an open panel builds no owned string
/// per input per frame.
fn value_str(ui: &mut Ui, binding: Option<&Binding>) -> TextInput<'static> {
    match binding {
        None => TextInput::Borrowed("—"),
        Some(Binding::Wire(_)) => TextInput::Borrowed("linked"),
        // `ConstValue::Display` — the same formatter a preview node's
        // value goes through via `DynamicValue::Display`, so a static
        // binding and a live one can't render one float two ways.
        Some(Binding::Const(v)) => TextInput::Interned(fmt!(ui, "{v}")),
    }
}

fn status_text(ui: &mut Ui, status: &ExecStatus) -> TextInput<'static> {
    match status {
        ExecStatus::None => TextInput::Borrowed("not run"),
        ExecStatus::Cached => TextInput::Borrowed("cached"),
        ExecStatus::Executed(secs) => {
            TextInput::Interned(fmt!(ui, "ran in {}", Elapsed::new(*secs)))
        }
        ExecStatus::Running(at) => TextInput::Interned(fmt!(
            ui,
            "running… {}",
            Elapsed::new(at.elapsed().as_secs_f64())
        )),
        ExecStatus::MissingInputs(_) => TextInput::Borrowed("missing inputs"),
        ExecStatus::Errored(_) => TextInput::Borrowed("errored"),
    }
}

/// Stable id for a node's inspector toggle chip in the header.
pub(crate) fn inspect_badge_wid(node_id: NodeId) -> WidgetId {
    wid::node("inspect_badge", node_id)
}

/// Stable id for a node's floating inspection panel.
pub(super) fn inspect_panel_wid(node_id: NodeId) -> WidgetId {
    wid::node("inspect_panel", node_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_label_dedups_the_type_and_reuses_the_scenes_own_handle() {
        use scenarium::FsPathConfig;
        use std::sync::Arc;

        let lib = Library::default();
        let path_type = DataType::FsPath(Arc::new(FsPathConfig::default()));

        // Type repeats the name (case-insensitively) → the name alone, no
        // "Image · Image" stutter; distinct type → "name · type". A path
        // port announces its type when the name differs, with no value
        // shown or with one.
        let cases: [(&str, &DataType, &str); 4] = [
            ("Float", &DataType::Float, "Float"),
            ("Brightness", &DataType::Float, "Brightness · float"),
            ("Path", &path_type, "Path"),
            ("Output", &path_type, "Output · path"),
        ];
        for (name, data_type, expected) in cases {
            let label = PortLabel::resolve(&lib, name, data_type);
            assert_eq!(label.to_string(), expected, "label for {name}");
            // The deduped case is the dominant one, and it must resolve to the
            // declaration's own `&str` with nothing appended: that is the whole
            // reason an open panel renders a port row straight from the scene's
            // own handles. Only the differing case carries a second piece.
            let deduped = expected == name;
            assert_eq!(
                label.type_name.is_none(),
                deduped,
                "{name}: a deduped label is the declared name alone, a \
                 combined one carries the type beside it",
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
