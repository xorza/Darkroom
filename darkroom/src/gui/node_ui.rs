use crate::document::{BoundarySide, GraphRef};
use crate::gui::breaker::BreakerProbe;
use crate::gui::graph_ui::node_ports;
use crate::gui::port_frame::PortFrame;
use crate::gui::value_editor;
use crate::gui::{PortKind, PortRef, UiAction};
use crate::intent::Intent;
use crate::scene::{InputBindingView, Scene, SceneNode};
use crate::theme::Theme;
use glam::Vec2;
use palantir::{
    Align, Background, Color, Configure, ContextMenu, Corners, HAlign, InternedStr, Key, MenuItem,
    Panel, Rect, Sense, Shadow, Shape, Shortcut, Sizing, Spacing, Stroke, Text, TextEdit,
    TextEditTheme, TextStyle, Ui, VAlign, WidgetId,
};
use scenarium::data::StaticValue;
use scenarium::graph::Binding;
use scenarium::prelude::{NodeBehavior, NodeId, SubgraphRef};
use std::collections::BTreeSet;

/// Read-only context the node-draw chain threads top to bottom: the
/// theme, the scene being rendered, and last frame's port geometry.
/// `Copy` (all shared refs), so it's passed by value — copying it while
/// a `&rcx.scene.nodes[..]` borrow is live is fine, which keeps
/// `draw_all`'s node loop borrow-clean. The mutable sinks (`out`,
/// `actions`) and the breaker `probe` stay separate params.
#[derive(Clone, Copy)]
pub(super) struct RecordCtx<'a> {
    pub theme: &'a Theme,
    pub scene: &'a Scene,
    pub port_frame: &'a PortFrame,
}

/// Owns rendering of every graph node plus the single active drag
/// anchor — the press-frame positions are snapshotted here so each
/// `MoveNodes` target is `start_pos + drag_delta`, not a running
/// integration over the moving source. Only one node can hold the
/// pointer at a time, so one anchor slot is enough.
///
/// `draw_all` is the single entry point; `GraphUI` calls it once per
/// frame after [`crate::gui::port_frame::PortFrame`] has been rebuilt
/// from last-frame's responses.
#[derive(Default, Debug)]
pub struct NodeUI {
    drag_anchor: Option<DragAnchor>,
}

#[derive(Clone, Debug)]
struct DragAnchor {
    /// The node the pointer latched. Keys the `response_for` lookup and
    /// the drag gesture; always present in `start_positions`.
    node_id: NodeId,
    /// Every node moving with this drag and its position at drag start:
    /// the whole selection when the grabbed node was already selected,
    /// else just the grabbed node. Each emits `start + delta`.
    start_positions: Vec<(NodeId, Vec2)>,
    /// Captured from the `drag_started` frame's `Response::widget_id()`
    /// so subsequent frames can `ui.response_for(widget_id)` *before*
    /// recording and bake the current `drag_delta` into `.position(...)`.
    /// Lets the node paint at the cursor's location in Pass A directly
    /// — no need to wait for Pass B's relayout to catch up.
    widget_id: WidgetId,
}

impl NodeUI {
    /// Iterate every scene node, recording its widget tree and
    /// pushing port circle centers into `centers`. Inserts into
    /// `port_nodes` only when every port resolved a layout rect.
    /// Emits an `Intent::MoveNodes` for any node holding an active
    /// LMB drag on its body (port circles capture their own clicks
    /// via `Sense::CLICK` so drags don't latch off the port grabs).
    pub(super) fn draw_all(
        &mut self,
        ui: &mut Ui,
        rcx: RecordCtx<'_>,
        probe: &mut BreakerProbe<'_>,
        out: &mut Vec<Intent>,
    ) {
        if let Some(b) = probe.state.as_deref_mut() {
            b.broken_nodes.clear();
        }
        for n in &rcx.scene.nodes {
            self.draw_one(ui, rcx, n, probe, out);
        }
        // Drop the anchor if its target node vanished from the graph
        // (mid-drag delete). Without this, the slot would linger and
        // could fire when a fresh node reused the id.
        if let Some(a) = &self.drag_anchor
            && !rcx.scene.nodes.iter().any(|n| n.id == a.node_id)
        {
            self.drag_anchor = None;
        }
    }

    fn draw_one(
        &mut self,
        ui: &mut Ui,
        rcx: RecordCtx<'_>,
        node: &SceneNode,
        probe: &mut BreakerProbe<'_>,
        out: &mut Vec<Intent>,
    ) {
        let theme = rcx.theme;

        // Probe last-frame's body rect (in canvas world coords) against
        // the breaker polyline. Hit → recolor border red and flag the
        // node for deletion on release. First-frame nodes have no rect
        // yet, so the breaker simply can't catch them until next frame
        // — acceptable: the user can't aim at something that hasn't
        // been painted.
        let body_rect = ui
            .response_for(node_widget_id(node.id))
            .layout_rect
            .map(|r| Rect {
                min: r.min - probe.origin,
                size: r.size,
            });
        let broken = match (probe.state.as_deref(), body_rect) {
            (Some(b), Some(r)) => b.intersects_rect(r),
            _ => false,
        };
        if broken {
            // unwrap: `broken == true` implies `state` is `Some`.
            probe
                .state
                .as_deref_mut()
                .unwrap()
                .broken_nodes
                .push(node.id);
        }
        let border = if broken {
            theme.connection_broken
        } else {
            theme.node_border
        };
        let selected = rcx.scene.selected_nodes.contains(&node.id);
        // Sample modifiers before the panel borrows `ui` for the rest
        // of this scope (the click handler below can't reborrow it).
        let shift_click = ui.modifiers().shift;
        // Soft halo behind the selected node. Zero-offset Gaussian
        // shadow so the glow wraps evenly; `spread` pushes the halo
        // out past the border so it reads at any zoom. Lives in the
        // `Background::shadow` slot so the encoder emits it on the
        // chrome branch — paints behind the node's fill in one
        // chrome batch, no extra `Shape::Shadow` overdraw bookkeeping.
        let shadow = if selected {
            Shadow {
                color: theme.selection_glow,
                offset: Vec2::ZERO,
                blur: 6.0,
                spread: 1.0,
                inset: false,
            }
        } else {
            Shadow::NONE
        };

        let panel = Panel::vstack()
            .id(node_widget_id(node.id))
            .position(node.pos)
            .min_size((theme.node_min_width, theme.node_min_height))
            .size((Sizing::Hug, Sizing::Hug))
            .sense(Sense::CLICK | Sense::DRAG)
            .background(Background {
                fill: theme.node_fill.into(),
                stroke: Stroke::solid(border, theme.node_border_width),
                corners: Corners::all(theme.node_corner_radius),
                shadow,
            })
            .show(ui, |ui| {
                header(ui, rcx, node, out);
                ports_row(ui, rcx, node, out);
            });
        let response = panel.response;

        // Click without drag → select. Plain click selects only this
        // node; Shift-click toggles its membership in the current
        // selection. `UndoStep::is_noop` filters a click that doesn't
        // change the set (e.g. clicking the sole selected node).
        if response.clicked() {
            out.push(select_intent(shift_click, rcx.scene, node.id));
        }

        // Latch the anchor on the press-frame edge; subsequent frames'
        // `prepass` peeks `response_for(widget_id)` before record runs
        // and converts `drag_delta` into a `MoveNodes` applied to
        // `Document` upstream of `Scene::rebuild`.
        if response.drag_started() {
            // Grabbing a node already in the selection drags the whole
            // group together; grabbing an unselected node selects only it
            // and drags it alone.
            let start_positions = if selected {
                rcx.scene
                    .nodes
                    .iter()
                    .filter(|n| rcx.scene.selected_nodes.contains(&n.id))
                    .map(|n| (n.id, n.pos))
                    .collect()
            } else {
                out.push(select_intent(false, rcx.scene, node.id));
                vec![(node.id, node.pos)]
            };
            self.drag_anchor = Some(DragAnchor {
                node_id: node.id,
                start_positions,
                widget_id: response.widget_id(),
            });
        }
    }

    /// Pre-record pass: peek palantir's input state for any widgets
    /// this `NodeUI` owns and push the corresponding `Intent`s into
    /// `out`. Runs before `Scene::rebuild` in `App::frame`, so any
    /// state mutation applied from these intents (notably drag-driven
    /// `MoveNodes`) lands in `Document` before recording — Pass A's
    /// arrange already reflects the cursor; no Pass B relayout retry.
    pub(super) fn prepass(&mut self, ui: &Ui, scene: &Scene, out: &mut Vec<Intent>) {
        // `node_id`/`widget_id` are `Copy`, so pull them out and drop the
        // borrow — that lets the early returns below reassign
        // `self.drag_anchor` without cloning the `start_positions` `Vec`,
        // which is only read in the success path (where the anchor isn't
        // cleared and can be re-borrowed).
        let Some(&DragAnchor {
            node_id, widget_id, ..
        }) = self.drag_anchor.as_ref()
        else {
            return;
        };
        // Drop a stale anchor whose node was removed last frame (e.g.
        // breaker swipe deleted the dragged node). Without this, the
        // emitted `MoveNodes` would target a missing node and panic in
        // `build_step`. `draw_all` also clears stale anchors, but only
        // after this prepass runs.
        if !scene.nodes.iter().any(|n| n.id == node_id) {
            self.drag_anchor = None;
            return;
        }
        let resp = ui.response_for(widget_id);
        // `drag_started` on a still-active anchor means a *new* gesture
        // just latched on the same widget — `record` will replace the
        // anchor this frame; emitting now with the stale start positions
        // makes the nodes snap to the previous gesture's start point.
        if resp.drag_started() {
            self.drag_anchor = None;
            return;
        }
        // No `drag_delta` means the drag isn't latched anymore (release
        // or pointer-left-surface). Drop the anchor so the next gesture
        // starts fresh.
        let Some(delta) = resp.drag_delta() else {
            self.drag_anchor = None;
            return;
        };
        // `drag_delta` is in screen pixels; node positions live in the
        // canvas's pre-transform frame. Divide by zoom so cursor travel
        // matches node travel at every zoom level.
        let zoom = if scene.zoom > 0.0 { scene.zoom } else { 1.0 };
        let offset = delta / zoom;
        // Anchor still present (success path never cleared it); re-borrow
        // to read the start positions without cloning.
        let to = self
            .drag_anchor
            .as_ref()
            .unwrap()
            .start_positions
            .iter()
            .map(|(id, start)| (*id, *start + offset))
            .collect();
        out.push(Intent::MoveNodes {
            grabbed: node_id,
            to,
        });
    }
}

/// Prepass scan: surface an `OpenGraph` for any subgraph node whose `S`
/// chip was clicked (read from last frame's response). Detecting the
/// open here — *before* the record — lets `App` switch the active graph
/// ahead of Pass A, so the subgraph records a pass earlier and its
/// connections draw with no first-frame gap. Linked subgraphs aren't
/// editable targets yet, so only `Local` opens.
pub(super) fn emit_subgraph_opens(ui: &Ui, scene: &Scene, actions: &mut Vec<UiAction>) {
    for n in &scene.nodes {
        if let Some(SubgraphRef::Local(id)) = n.subgraph
            && ui.response_for(subgraph_badge_wid(n.id)).clicked
        {
            actions.push(UiAction::OpenGraph(GraphRef::Local(id)));
        }
    }
}

/// Prepass scan: port-circle double-clicks read from last frame's
/// responses. An input double-click clears that input's binding; an
/// output double-click disconnects every consumer it feeds.
///
/// Emitted pre-record (like the connection commit) because clearing a
/// `Const` input removes its inline editor and resizes the node — doing it
/// before Pass A lets the node arrange at its settled size and the wires
/// re-anchor the same frame, instead of floating until the relayout pass.
pub(super) fn emit_port_disconnects(ui: &Ui, scene: &Scene, out: &mut Vec<Intent>) {
    for node in &scene.nodes {
        for port in node_ports(scene, node, PortKind::Input) {
            if ui.response_for(port_circle_wid(port)).double_clicked() {
                out.push(set_input(port, Binding::None));
            }
        }
        for port in node_ports(scene, node, PortKind::Output) {
            if ui.response_for(port_circle_wid(port)).double_clicked() {
                // An output may feed many inputs — clear each consumer.
                for c in &scene.connections {
                    if c.src_node == port.node_id && c.src_port == port.port_idx {
                        out.push(set_input(
                            PortRef {
                                node_id: c.tgt_node,
                                kind: PortKind::Input,
                                port_idx: c.tgt_port,
                            },
                            Binding::None,
                        ));
                    }
                }
            }
        }
    }
}

/// Stable widget id for the node's outer body panel. Derived from
/// the domain `NodeId` so `response_for` can probe last-frame's
/// arranged rect (used by the connection breaker's body-hit test)
/// without needing the panel's response to round-trip first.
pub(super) fn node_widget_id(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.body", node_id))
}

/// Side of a header indicator chip (px), and its glyph font size.
const BADGE_SIZE: f32 = 15.0;
const BADGE_FONT: f32 = 10.0;

fn header(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let theme = rcx.theme;
    let r = theme.header_corner_radius;
    Panel::hstack()
        .id_salt("header")
        .size((Sizing::FILL, Sizing::Hug))
        .padding(Spacing::xy(8.0, 4.0))
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .background(Background {
            fill: theme.header_fill.into(),
            corners: Corners::new(r, r, 0.0, 0.0),
            ..Default::default()
        })
        .show(ui, |ui| {
            Text::new(node.name.clone()).show(ui);
            // FILL spacer pushes the badge cluster to the right edge.
            Panel::hstack()
                .id_salt("badge_spacer")
                .size((Sizing::FILL, Sizing::Hug))
                .show(ui, |_| {});
            // Subgraph chip is the open-in-tab affordance. We only *draw*
            // it here (with its stable id); the click is read next frame
            // in `emit_subgraph_opens` (prepass) so the open applies
            // before the record — letting the subgraph record a pass
            // earlier and its connections draw with no first-frame gap.
            if node.subgraph.is_some() {
                badge(
                    ui,
                    theme,
                    "badge_sg",
                    "S",
                    theme.badge_subgraph,
                    true,
                    Some(subgraph_badge_wid(node.id)),
                );
            }
            if node.terminal {
                badge(ui, theme, "badge_t", "T", theme.badge_terminal, true, None);
            }
            let toggled = badge(
                ui,
                theme,
                "badge_c",
                "C",
                theme.badge_cache,
                node.cached,
                Some(cache_badge_wid(node.id)),
            );
            if toggled {
                out.push(Intent::SetCacheBehavior {
                    node_id: node.id,
                    to: if node.cached {
                        NodeBehavior::AsFunction
                    } else {
                        NodeBehavior::Once
                    },
                });
            }
        });
}

/// Stable id for a node's clickable cache-toggle chip. Domain-derived
/// so `response_for` works without threading state.
fn cache_badge_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.cache_badge", node_id))
}

/// Stable id for a subgraph node's clickable open-in-tab chip.
fn subgraph_badge_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.subgraph_badge", node_id))
}

/// One header indicator chip: a small rounded square with a centered
/// glyph. `filled` paints a solid swatch (active/descriptor); otherwise
/// it's a hollow outline (inactive toggle). A `wid` makes it clickable
/// and the returned bool reports a click this frame; decorative chips
/// pass `None` and ignore the result.
fn badge(
    ui: &mut Ui,
    theme: &Theme,
    salt: &'static str,
    glyph: &'static str,
    color: Color,
    filled: bool,
    wid: Option<WidgetId>,
) -> bool {
    let background = if filled {
        Background {
            fill: color.into(),
            corners: Corners::all(3.0),
            ..Default::default()
        }
    } else {
        Background {
            stroke: Stroke::solid(color, 1.0),
            corners: Corners::all(3.0),
            ..Default::default()
        }
    };
    // Solid chips carry dark glyphs (contrast against the swatch);
    // hollow chips ink the glyph in the accent itself.
    let glyph_color = if filled { theme.header_fill } else { color };
    let mut panel = Panel::zstack()
        .size((Sizing::Fixed(BADGE_SIZE), Sizing::Fixed(BADGE_SIZE)))
        .child_align(Align::CENTER)
        .background(background);
    panel = match wid {
        Some(w) => panel.id(w).sense(Sense::CLICK),
        None => panel.id_salt(salt),
    };
    panel
        .show(ui, |ui| {
            Text::new(glyph)
                .style(TextStyle {
                    color: glyph_color,
                    font_size_px: BADGE_FONT,
                    ..ui.theme.text
                })
                .show(ui);
        })
        .response
        .clicked()
}

fn ports_row(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let theme = rcx.theme;
    Panel::hstack()
        .id_salt("ports")
        .size((Sizing::FILL, Sizing::Hug))
        .padding(Spacing::xy(theme.port_col_pad_x, 0.0))
        .gap(theme.port_cols_gap)
        .show(ui, |ui| {
            input_column(ui, rcx, node, out);
            output_column(ui, rcx, node, out);
        });
}

fn input_column(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let names = rcx.scene.ports(node.inputs);
    let bindings = rcx.scene.bindings(node.input_bindings);
    let defaults = rcx.scene.defaults(node.input_bindings);
    let theme = rcx.theme;
    // Boundary (`SubgraphInput`/`SubgraphOutput`) ports route the
    // interface, not literal values — no const affordance.
    let allow_const = !node.boundary;
    Panel::vstack()
        .id_salt("in")
        .size((Sizing::Fill(1.0), Sizing::Hug))
        .padding(Spacing::new(
            0.0,
            theme.port_col_pad_top,
            0.0,
            theme.port_col_pad_top,
        ))
        .gap(theme.port_gap)
        .child_align(Align::h(HAlign::Left))
        .show(ui, |ui| {
            for (i, name) in names.iter().enumerate() {
                let port = PortRef {
                    node_id: node.id,
                    kind: PortKind::Input,
                    port_idx: i,
                };
                let binding = bindings.get(i).unwrap_or(&InputBindingView::None);
                let default = defaults.get(i).cloned();
                // A `SubgraphOutput` boundary node's input ports are the
                // subgraph's *outputs* — renameable, except the trailing
                // "+" placeholder.
                let rename = (node.boundary && i + 1 < names.len()).then_some(BoundarySide::Output);
                input_port_row(
                    ui,
                    rcx,
                    port,
                    name.clone(),
                    binding,
                    default,
                    allow_const,
                    rename,
                    out,
                );
            }
        });
}

fn output_column(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let names = rcx.scene.ports(node.outputs);
    let theme = rcx.theme;
    Panel::vstack()
        .id_salt("out")
        .size((Sizing::Fill(1.0), Sizing::Hug))
        .padding(Spacing::new(
            0.0,
            theme.port_col_pad_top,
            0.0,
            theme.port_col_pad_top,
        ))
        .gap(theme.port_gap)
        .child_align(Align::h(HAlign::Right))
        .show(ui, |ui| {
            for (i, name) in names.iter().enumerate() {
                let port = PortRef {
                    node_id: node.id,
                    kind: PortKind::Output,
                    port_idx: i,
                };
                // A `SubgraphInput` boundary node's output ports are the
                // subgraph's *inputs* — renameable, except the trailing
                // "+" placeholder.
                let rename = (node.boundary && i + 1 < names.len()).then_some(BoundarySide::Input);
                output_port_row(ui, rcx, port, name.clone(), rename, out);
            }
        });
}

/// Stable widget id for one port circle. Derived from
/// `(node_id, kind, port_idx)` so prepass can look up
/// `response_for(port_circle_wid(..))` without threading the cache —
/// every port's id is reconstructible from its domain coordinates.
pub(super) fn port_circle_wid(port: PortRef) -> WidgetId {
    WidgetId::from_hash((
        "graph.node.port_circle",
        port.node_id,
        port.kind as u8,
        port.port_idx,
    ))
}

/// One output port = label + circle, vertically centered. Circle has a
/// negative right margin so it overhangs the column. The circle's
/// `WidgetId` is the deterministic `port_circle_wid(port)`, so
/// downstream consumers (`PortFrame::rebuild`, snap, draw) reconstruct
/// it from domain coords without threading any cache.
fn output_port_row(
    ui: &mut Ui,
    rcx: RecordCtx<'_>,
    port: PortRef,
    name: InternedStr,
    rename: Option<BoundarySide>,
    out: &mut Vec<Intent>,
) {
    let theme = rcx.theme;
    let fill = if rcx.port_frame.is_hovered(port) {
        theme.output_port_hover
    } else {
        theme.output_port
    };
    let wid = port_circle_wid(port);
    let overhang = theme.port_radius() + theme.port_col_pad_x;
    Panel::hstack()
        .id_salt(("port", port.port_idx))
        .size((Sizing::Hug, Sizing::Hug))
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            port_label(ui, rcx, port, name, rename, out);
            circle_frame(ui, theme, wid, fill, Spacing::new(0.0, 0.0, -overhang, 0.0));
        });
    // Double-click to disconnect every consumer is handled in
    // `emit_port_disconnects` (prepass) alongside the input-side gesture.
}

#[allow(clippy::too_many_arguments)]
fn input_port_row(
    ui: &mut Ui,
    rcx: RecordCtx<'_>,
    port: PortRef,
    name: InternedStr,
    binding: &InputBindingView,
    default_static: Option<StaticValue>,
    allow_const: bool,
    rename: Option<BoundarySide>,
    out: &mut Vec<Intent>,
) {
    let theme = rcx.theme;
    let fill = if rcx.port_frame.is_hovered(port) {
        theme.input_port_hover
    } else {
        theme.input_port
    };
    let overhang = theme.port_radius() + theme.port_col_pad_x;
    let margin = Spacing::new(-overhang, 0.0, 0.0, 0.0);
    let wid = port_circle_wid(port);
    let editor_w = theme.value_editor_width;
    let row = Panel::hstack()
        .id_salt(("port", port.port_idx))
        .size((Sizing::Hug, Sizing::Hug))
        .sense(Sense::CLICK)
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            circle_frame(ui, theme, wid, fill, margin);
            port_label(ui, rcx, port, name.clone(), rename, out);
            if allow_const && let InputBindingView::Const(value) = binding {
                let editor_id =
                    WidgetId::from_hash(("graph.node.const_editor", port.node_id, port.port_idx));
                if let Some(new_value) = value_editor::show(ui, editor_id, value, editor_w) {
                    out.push(set_input(port, Binding::Const(new_value)));
                }
            }
        });
    // Open on right-click anywhere on the row — circle, label, or
    // editor. The circle has its own `Sense::CLICK` and consumes hits
    // over its rect, so the row's snapshot alone misses clicks landing
    // on the circle (no event bubbling in palantir's hit-test).
    let menu_id = row.response.widget_id();
    let row_secondary = row.response.secondary_clicked();
    let circle_state = ui.response_for(wid);
    if (row_secondary || circle_state.secondary_clicked)
        && let Some(p) = ui.pointer_pos()
    {
        ContextMenu::open(ui, menu_id, p);
    }
    // Double-click on the circle clears the binding — handled in
    // `emit_port_disconnects` (prepass), since clearing a `Const` resizes
    // the node and the wires must re-anchor before the record.
    ContextMenu::for_id(menu_id)
        .size((Sizing::Hug, Sizing::Hug))
        .show(ui, |ui, popup| {
            let can_set = allow_const
                && !matches!(binding, InputBindingView::Const(_))
                && default_static.is_some();
            if MenuItem::new("Set constant")
                .enabled(can_set)
                .show(ui, popup)
                .clicked()
                && let Some(value) = default_static.clone()
            {
                out.push(set_input(port, Binding::Const(value)));
            }
            if MenuItem::new("Clear binding")
                .enabled(!matches!(binding, InputBindingView::None))
                .show(ui, popup)
                .clicked()
            {
                out.push(set_input(port, Binding::None));
            }
        });
}

pub(super) fn set_input(port: PortRef, to: Binding) -> Intent {
    Intent::SetInput {
        node_id: port.node_id,
        input_idx: port.port_idx,
        to,
    }
}

/// The `SetSelection` a click on `node_id` produces: plain click selects
/// only it, Shift-click toggles its membership. Shared by the node body
/// and the port labels so clicking a label selects the node like the
/// body does. `UndoStep::is_noop` drops the entry when nothing changed.
fn select_intent(shift: bool, scene: &Scene, node_id: NodeId) -> Intent {
    let mut to = if shift {
        scene.selected_nodes.clone()
    } else {
        BTreeSet::new()
    };
    if shift && scene.selected_nodes.contains(&node_id) {
        to.remove(&node_id);
    } else {
        to.insert(node_id);
    }
    Intent::SetSelection { to }
}

/// Character cap for a boundary-port name in the inline rename editor.
const PORT_NAME_MAX_CHARS: usize = 24;

/// Cross-frame state for a boundary port's inline rename editor, held in
/// palantir's `StateMap` under the editor's `WidgetId`.
#[derive(Default, Clone)]
struct PortRename {
    active: bool,
    /// Latches once the editor actually holds focus, so the frames
    /// between `request_focus` and focus landing don't read as a blur
    /// and commit early.
    focused_once: bool,
    draft: String,
}

/// Stable id for a port's rename editor — and for the sensing label
/// panel shown when idle, so the same `WidgetId` is recorded every frame
/// across the label⇄editor swap (palantir drops state rows for ids it
/// doesn't see).
fn port_rename_wid(port: PortRef) -> WidgetId {
    WidgetId::from_hash((
        "graph.node.port_rename",
        port.node_id,
        port.kind as u8,
        port.port_idx,
    ))
}

/// A port label. When `rename` is `Some`, double-clicking swaps the
/// label for a fixed-width, length-capped `TextEdit`; Enter or focus
/// loss commits a [`Intent::RenameBoundaryPort`], Esc cancels. `None`
/// (regular node ports and the trailing "+" placeholder) renders plain
/// text.
fn port_label(
    ui: &mut Ui,
    rcx: RecordCtx<'_>,
    port: PortRef,
    name: InternedStr,
    rename: Option<BoundarySide>,
    out: &mut Vec<Intent>,
) {
    let theme = rcx.theme;
    let Some(side) = rename else {
        Text::new(name).show(ui);
        return;
    };
    let id = port_rename_wid(port);
    // darkroom port names are always `Owned` (built via `String::into`),
    // so `as_str` resolves without a live text arena. Resolve lazily —
    // only the double-click seed and the commit compare need it, never
    // the idle per-frame path.
    if !ui.state_mut::<PortRename>(id).active {
        let shift = ui.modifiers().shift;
        let resp = Panel::hstack()
            .id(id)
            .size((Sizing::Hug, Sizing::Hug))
            .sense(Sense::CLICK)
            .show(ui, |ui| {
                // `InternedStr::clone` is allocation-free for the `Owned`
                // names darkroom builds, so this is cheap per frame.
                Text::new(name.clone()).show(ui);
            })
            .response;
        // Single click selects the node (the label otherwise swallows
        // the click the body would have gotten); double-click renames.
        if resp.clicked() {
            out.push(select_intent(shift, rcx.scene, port.node_id));
        }
        if resp.double_clicked() {
            let st = ui.state_mut::<PortRename>(id);
            st.active = true;
            st.focused_once = false;
            st.draft = name.as_str("").to_owned();
            ui.request_focus(Some(id));
        }
        return;
    }

    let mut draft = std::mem::take(&mut ui.state_mut::<PortRename>(id).draft);
    TextEdit::new(&mut draft)
        .id(id)
        .style(flat_edit_style(ui))
        .max_chars(PORT_NAME_MAX_CHARS)
        .size((Sizing::Fixed(theme.value_editor_width), Sizing::Hug))
        .show(ui);
    let focused = ui.focused_id() == Some(id);
    let escape = ui.escape_pressed();
    let enter = ui.key_pressed(Shortcut::key(Key::Enter));
    let commit = {
        let st = ui.state_mut::<PortRename>(id);
        st.draft = draft.clone();
        st.focused_once |= focused;
        // Commit on Enter or on blur (once focus had landed); Esc wins
        // as a cancel.
        !escape && (enter || (st.focused_once && !focused))
    };
    if commit || escape {
        if commit && draft != name.as_str("") {
            out.push(Intent::RenameBoundaryPort {
                side,
                idx: port.port_idx,
                to: draft,
            });
        }
        let st = ui.state_mut::<PortRename>(id);
        st.active = false;
        st.focused_once = false;
        ui.request_focus(None);
    }
}

/// The ambient text-edit theme flattened for an inline port-rename
/// field: zero padding/margin and no border, so the editor's `Hug`
/// height equals the plain `Text` label's line height — the node body
/// doesn't grow when a label enters/exits edit mode. The fill stays, so
/// the fixed-width field still reads as editable.
fn flat_edit_style(ui: &Ui) -> TextEditTheme {
    let mut style = ui.theme.text_edit.clone();
    style.padding = Spacing::ZERO;
    style.margin = Spacing::ZERO;
    for look in [&mut style.normal, &mut style.focused, &mut style.disabled] {
        if let Some(bg) = look.background.as_mut() {
            bg.stroke = Stroke::ZERO;
        }
    }
    style
}

/// Hover / grab box scaled past the painted dot so ports are easier to
/// hit and snap to, while the visible circle stays `port_size`.
const PORT_HIT_SCALE: f32 = 1.8;

fn circle_frame(ui: &mut Ui, theme: &Theme, wid: WidgetId, fill: Color, margin: Spacing) {
    let port = theme.port_size;
    let hit = port * PORT_HIT_SCALE;
    let inset = (hit - port) * 0.5;

    // The sensing element is `hit`-sized, but the extra (`inset` on each
    // side) is pulled back out of the layout with negative margin, so
    // node layout and the dot's position are unchanged — only the
    // hover/grab area grows. The dot itself paints as a centered shape.
    let [l, t, r, b] = margin.as_array();
    let hit_margin = Spacing::new(l - inset, t - inset, r - inset, b - inset);
    let radius = theme.port_radius();

    // Explicit `id(wid)` so the cross-frame id stays stable: prepass
    // computes the same `port_circle_wid` and reads its response,
    // record paints with the same id — no drift even if the parent
    // structure shifts. CLICK | DRAG so the port (a) intercepts the
    // press before it falls through to the node body's `Sense::DRAG`,
    // and (b) can latch a connection drag.
    Panel::zstack()
        .id(wid)
        .size((Sizing::Fixed(hit), Sizing::Fixed(hit)))
        .margin(hit_margin)
        .sense(Sense::CLICK | Sense::DRAG)
        .show(ui, |ui| {
            ui.add_shape(Shape::RoundedRect {
                local_rect: Some(Rect::new(inset, inset, port, port)),
                corners: Corners::all(radius),
                fill: fill.into(),
                stroke: Stroke::ZERO,
            });
        });
}
