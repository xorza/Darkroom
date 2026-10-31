//! [`TieMenuUi`]: the menu a right-click on a tie opens. `darkroom/AGENTS.md`, "Wire ties", "One
//! wire leaves through the tie's menu", holds the decision.

use palantir::{InternedStr, MenuItem, MenuSeparator, Ui, fmt};

use crate::core::document::tie::{TieAngle, TieId};
use crate::core::document::wire_key::{DataWire, WireKey};
use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::frame::requests::Requests;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph_ui::frame::tie_layout::TieLayout;
use crate::gui::pane::graph_ui::gesture::item_menu::ItemMenu;
use crate::gui::pane::graph_ui::gesture::tie_ui::tie_widget::{AngleSnap, knob_angle};

/// Right-click on a tie: release one of its wires, pin or free its angle, or remove it. A
/// Release entry under the pointer lights its strand, so the pick names the wire it shows.
#[derive(Default, Debug)]
pub(crate) struct TieMenuUi {
    menu: ItemMenu<TieId>,
    /// The wire of the Release entry under the pointer, which paints lit.
    lit: Option<WireKey>,
}

/// A menu pick before it becomes an edit of the tie the menu opened on.
#[derive(Clone, Copy, Debug)]
enum MenuChoice {
    Release(WireKey),
    Pin,
    Free,
    Remove,
}

impl TieMenuUi {
    pub(crate) const fn reset(&mut self) {
        self.menu.reset();
        self.lit = None;
    }

    pub(crate) const fn in_flight(&self) -> bool {
        self.menu.is_open()
    }

    /// The wire a hovered Release entry names.
    pub(crate) const fn lit(&self) -> Option<WireKey> {
        self.lit
    }

    /// Open the menu on `tie`, which the record pass just saw right-clicked; see
    /// [`ItemMenu::open_on`]. Returns the selection edit, when it needs one.
    pub(crate) fn open_on(
        &mut self,
        ui: &mut Ui,
        graph_ctx: GraphCtx<'_>,
        tie: TieId,
    ) -> Option<GraphIntent> {
        self.menu.open_on(ui, graph_ctx, tie)
    }

    /// Record the menu and turn this frame's pick into its intent. `ties` is the last pass's
    /// layout, whose band angle a Pin keeps.
    pub(crate) fn apply(
        &mut self,
        ui: &mut Ui,
        ctx: CanvasCtx<'_>,
        ties: &TieLayout,
        out: &mut Requests,
    ) {
        let graph_ctx = ctx.graph_ctx();
        let view = graph_ctx.view();
        let mut lit = None;
        self.menu.retain(|tie| view.ties.contains_key(&tie));
        let pick = self.menu.show(ui, "tie_menu", |ui, popup, tie| {
            let mut chosen = None;
            for (&wire, route) in &view.routes {
                if !route.contains(tie) {
                    continue;
                }
                let label = release_label(ui, graph_ctx, wire);
                let entry = MenuItem::new(label).show(ui, popup);
                if entry.hovered() {
                    lit = Some(wire);
                }
                if entry.clicked() {
                    chosen = Some(MenuChoice::Release(wire));
                }
            }
            MenuSeparator::new().show(ui);
            let pinned = view
                .ties
                .get(&tie)
                .is_some_and(|placed| matches!(placed.angle, TieAngle::Pinned(_)));
            let (label, choice) = if pinned {
                ("Free angle", MenuChoice::Free)
            } else {
                ("Pin angle", MenuChoice::Pin)
            };
            if MenuItem::new(label).show(ui, popup).clicked() {
                chosen = Some(choice);
            }
            MenuSeparator::new().show(ui);
            if MenuItem::new("Remove tie").show(ui, popup).clicked() {
                chosen = Some(MenuChoice::Remove);
            }
            chosen
        });
        self.lit = lit;
        let Some(pick) = pick else {
            return;
        };
        let tie = pick.target;
        let angle = |angle| GraphIntent::SetTieAngle {
            tie,
            angle,
            gesture: None,
        };
        let intent = match pick.choice {
            MenuChoice::Release(wire) => GraphIntent::ReleaseWire { tie, wire },
            // Pinned where the band stands now, so pinning moves nothing.
            MenuChoice::Pin => {
                let Some(band) = ties.band(tie) else {
                    return;
                };
                angle(TieAngle::Pinned(knob_angle(band.axis, AngleSnap::Free)))
            }
            MenuChoice::Free => angle(TieAngle::Auto),
            MenuChoice::Remove => GraphIntent::RemoveTie { tie },
        };
        out.push_graph(intent);
    }
}

/// A Release entry's label: `<producer>.<output> → <consumer>.<input>` for a data wire,
/// `<emitter>.<event> → <subscriber>` for an event wire. The load holds a routed wire to both of
/// its ends and their ports, so each lookup holds.
fn release_label(ui: &mut Ui, graph_ctx: GraphCtx<'_>, wire: WireKey) -> InternedStr {
    let node = |node_id| {
        graph_ctx
            .node(node_id)
            .expect("a routed wire's end node exists")
    };
    match wire {
        WireKey::Data(DataWire { output, input }) => {
            let (from, to) = (node(output.node_id), node(input.node_id));
            fmt!(
                ui,
                "Release {}.{} → {}.{}",
                from.name(),
                from.outputs()
                    .nth(output.port_idx)
                    .expect("a routed wire's output exists")
                    .name(),
                to.name(),
                to.input(input.port_idx)
                    .expect("a routed wire's input exists")
                    .name(),
            )
        }
        WireKey::Event(subscription) => {
            let (from, to) = (
                node(subscription.event.node_id),
                node(subscription.subscriber),
            );
            fmt!(
                ui,
                "Release {}.{} → {}",
                from.name(),
                from.events()
                    .get(subscription.event.event_idx)
                    .expect("a routed subscription's event exists")
                    .name,
                to.name(),
            )
        }
    }
}
