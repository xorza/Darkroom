//! [`Gestures`]: the canvas's controllers, and what the frame's input means among them.

use std::mem;

use palantir::{Key, Modifiers, PointerAction, PointerButton, PointerEdge, Ui};
use scenarium::NodeId;

use crate::core::document::item_id::ItemId;
use crate::core::document::tie::TieId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::click_mode::ClickMode;
use crate::gui::frame::stage::Stage;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph_ui::canvas_input::{
    CanvasGesture, CanvasInput, Escape, SecondaryPress,
};
use crate::gui::pane::graph_ui::gesture::breaker_ui::BreakerUi;
use crate::gui::pane::graph_ui::gesture::group_drag::{self, GroupDrag};
use crate::gui::pane::graph_ui::gesture::new_node_ui::NewNodeUi;
use crate::gui::pane::graph_ui::gesture::node_menu_ui::NodeMenuUi;
use crate::gui::pane::graph_ui::gesture::pan_zoom::camera_gesture::CameraGesture;
use crate::gui::pane::graph_ui::gesture::preview_drag;
use crate::gui::pane::graph_ui::gesture::selection_ui::SelectionUi;
use crate::gui::pane::graph_ui::gesture::subscription_ui::SubscriptionUi;
use crate::gui::pane::graph_ui::gesture::tie_ui::TieUi;
use crate::gui::pane::graph_ui::gesture::tie_ui::tie_menu_ui::TieMenuUi;
use crate::gui::pane::graph_ui::gesture::tie_ui::tie_widget::{self, TieWidgets};
use crate::gui::pane::graph_ui::gesture::wire_ui::{WireEnd, WireUi};
use crate::gui::pane::graph_ui::unsettled_ctx::UnsettledCtx;
use crate::gui::pane::graph_ui::wid;

/// The controllers of the gestures in flight, which the canvas drives once a frame on the input
/// stage, and whose popups and previews it records. A group apart from the geometry, so one borrows
/// mutably beside a shared borrow of the other, with no context built by hand.
///
/// Refused: a `Controller` trait, as the set is closed and the order of calls is the design.
/// Replacing the group on a reset, which throws away the buffers each grows, so each knows which of
/// its state is a buffer and which is the gesture.
#[derive(Debug, Default)]
pub(crate) struct Gestures {
    /// Whether the right press now held, or released this frame, already ended a floating wire,
    /// whose release then opens no palette: palantir reports the click on the release, frames
    /// after the press that ended the wire.
    secondary: SecondaryPress,
    /// The one group drag a pointer holds, latched off a node's, a tie's or a preview spawn's
    /// press.
    pub(crate) group_drag: GroupDrag,
    pub(crate) breaker_ui: BreakerUi,
    pub(crate) wire_ui: WireUi,
    pub(crate) subscription_ui: SubscriptionUi,
    pub(crate) new_node_ui: NewNodeUi,
    pub(crate) node_menu: NodeMenuUi,
    pub(crate) selection_ui: SelectionUi,
    pub(crate) tie_ui: TieUi,
    pub(crate) tie_widgets: TieWidgets,
    pub(crate) tie_menu: TieMenuUi,
    /// The camera's input in flight: a pan drag's latch, and the run of
    /// wheel or pinch input. Input bookkeeping, not viewport state.
    pub(crate) camera: CameraGesture,
    /// The frame's edges, a field so each frame reuses one buffer's capacity: at most three a
    /// button.
    actions: Vec<PointerAction>,
}

impl Gestures {
    /// Drop every in-flight gesture, keeping the buffers the controllers grow.
    ///
    /// Destructured rather than assigned wholesale so the compiler makes the call: a field added
    /// here does not compile until it is either dropped or named as surviving.
    pub(crate) fn reset(&mut self) {
        let Self {
            secondary,
            group_drag,
            breaker_ui,
            wire_ui,
            subscription_ui,
            new_node_ui,
            node_menu,
            selection_ui,
            tie_ui,
            tie_widgets,
            tie_menu,
            camera,
            actions: _,
        } = self;
        breaker_ui.reset();
        wire_ui.reset();
        group_drag.reset();
        subscription_ui.reset();
        new_node_ui.reset();
        node_menu.reset();
        selection_ui.reset();
        tie_ui.reset();
        tie_widgets.reset();
        tie_menu.reset();
        camera.reset();
        *secondary = SecondaryPress::Free;
    }

    /// Whether a gesture, a menu or the palette is in flight — what Esc
    /// cancels before it would deselect.
    ///
    /// Destructured like [`Self::reset`], so a new controller does not compile until it is asked
    /// here too.
    pub(crate) const fn in_flight(&self) -> bool {
        let Self {
            secondary: _,
            group_drag,
            breaker_ui,
            wire_ui,
            subscription_ui,
            new_node_ui,
            node_menu,
            selection_ui,
            tie_ui,
            tie_widgets,
            tie_menu,
            camera,
            actions: _,
        } = self;
        group_drag.in_flight()
            || breaker_ui.in_flight()
            || wire_ui.in_flight()
            || subscription_ui.in_flight()
            || new_node_ui.in_flight()
            || node_menu.in_flight()
            || selection_ui.in_flight()
            || tie_ui.in_flight()
            || tie_widgets.in_flight()
            || tie_menu.in_flight()
            || camera.in_flight()
    }

    /// What this frame's input means on the canvas, read once ahead of every controller.
    ///
    /// Esc cancels what is in flight, whatever the modifiers: Ctrl still held from a spawn drag's
    /// chord must not swallow the cancel. Deselect, a command, is plain Esc alone. Both read every
    /// frame, so each stays watched.
    pub(crate) fn read_input(&self, ui: &mut Ui) -> CanvasInput {
        let cancel = ui.key_pressed(Key::Escape);
        let plain = ui.shortcut_pressed(Key::Escape);
        let escape = if self.in_flight() && cancel {
            Escape::Cancels
        } else if plain {
            Escape::Deselects
        } else {
            Escape::Unpressed
        };
        let canvas = ui.response_for(canvas::outer_wid());
        CanvasInput {
            gesture: CanvasGesture::classify(&canvas, ui.modifiers(), self.secondary),
            escape,
        }
    }

    /// The controllers that run ahead of the geometry's rebuild, as none reads it.
    pub(crate) fn before_geometry(&mut self, ui: &mut Ui, stage: &mut Stage<'_, UnsettledCtx<'_>>) {
        self.camera.input(ui, stage);
        self.group_drag.input(ui, stage);
        self.tie_widgets.input(ui, stage);
    }

    /// The controllers that read the settled geometry, each edge of the frame acted on.
    pub(crate) fn input(&mut self, ui: &mut Ui, stage: &mut Stage<'_, CanvasCtx<'_>>) {
        // Both port-drag claimants sit after the rebuild so they read this frame's drag edges and
        // centers, and `preview_drag::modifier_held` keeps them disjoint: the preview spawn takes
        // the output column under the chord, the wire gesture takes it otherwise.
        if !self.group_drag.in_flight()
            && let Some(start) = preview_drag::start(ui, stage.ctx)
        {
            self.group_drag.latch(start, ui, stage.out);
        }
        let canvas = ui.response_for(canvas::outer_wid());
        let dropped = match self.wire_ui.input(ui, stage) {
            Some(WireEnd::DroppedOnCanvas(source)) => Some(source),
            // The press that ended a floating wire is spent, its release too when it lands this
            // same frame: the palette sees no click.
            Some(WireEnd::Secondary) => {
                self.secondary = SecondaryPress::Spent;
                let modifiers = ui.modifiers();
                let gesture = CanvasGesture::classify(&canvas, modifiers, self.secondary);
                stage.ctx = stage.ctx.with_gesture(gesture);
                None
            }
            None => None,
        };
        // Past the classifications that read it, so a release still spends the press it ends.
        if !canvas.right.held() {
            self.secondary = SecondaryPress::Free;
        }
        // Subscription wires (emitter → subscriber) latch and commit here, for the same reasons as
        // the wire gesture above; an emitter glyph and a data port can't both latch (different
        // widget-id spaces).
        self.subscription_ui.input(ui, stage);
        self.selection_ui.input(ui, stage);
        self.breaker_ui.input(ui, stage);
        self.tie_ui.input(ui, stage);
        self.new_node_ui.open(ui, stage.ctx, dropped);

        let mut actions = mem::take(&mut self.actions);
        actions.clear();
        actions.extend(ui.pointer_actions());
        for &action in &actions {
            self.act_on(ui, action, stage);
        }
        self.actions = actions;
    }

    /// Act on `action` when it is the canvas's: a click on the bare canvas, and a click or a drag's
    /// latch on a node's handle, a tie or its knob. A click on a node's title, chip or field is its
    /// widget's.
    fn act_on(&mut self, ui: &mut Ui, action: PointerAction, stage: &mut Stage<'_, CanvasCtx<'_>>) {
        if action.id == canvas::outer_wid() {
            deselect_on_click(action, stage);
            return;
        }
        let view = stage.ctx.graph_ctx().view();
        if let Some(&tie) = view
            .ties
            .keys()
            .find(|&&tie| wid::tie_body(tie) == action.id)
        {
            self.act_on_tie(ui, tie, action, stage);
            return;
        }
        if let Some(&tie) = view
            .ties
            .keys()
            .find(|&&tie| wid::tie_knob(tie) == action.id)
        {
            self.tie_widgets.act_on_knob(tie, action, stage);
            return;
        }
        let handles = |node: NodeId| wid::drag_handles(node).any(|handle| handle == action.id);
        if let Some(node) = view
            .node_placements
            .keys()
            .copied()
            .find(|&node| handles(node))
        {
            self.act_on_node(ui, node, action, stage);
        }
    }

    /// On a body, a left click selects, Shift toggles and `UndoStep::is_noop` filters a click that
    /// changes nothing, and a right click opens its menu. A left drag on either handle grabs the
    /// node.
    fn act_on_node(
        &mut self,
        ui: &mut Ui,
        node: NodeId,
        action: PointerAction,
        stage: &mut Stage<'_, CanvasCtx<'_>>,
    ) {
        let graph_ctx = stage.ctx.graph_ctx();
        let on_body = action.id == wid::body(node);
        match (action.edge, action.button) {
            (PointerEdge::Clicked { .. }, PointerButton::Left) if on_body => {
                let mode = ClickMode::with_shift(ui.modifiers().contains(Modifiers::SHIFT));
                stage.out.extend_graph(GraphIntent::click(
                    mode,
                    graph_ctx.selected(),
                    ItemId::Node(node),
                ));
            }
            (PointerEdge::Clicked { .. }, PointerButton::Right) if on_body => {
                let opened = self.node_menu.open_on(ui, graph_ctx, node);
                stage.out.extend_graph(opened);
            }
            (PointerEdge::DragStarted, PointerButton::Left) => {
                let node = graph_ctx
                    .node(node)
                    .expect("the placements list the context's own nodes");
                let start = group_drag::node_grab(node, action.id);
                self.group_drag.latch(start, ui, stage.out);
            }
            _ => {}
        }
    }

    /// A left click selects the tie, a right click opens its menu, and a left drag grabs it.
    fn act_on_tie(
        &mut self,
        ui: &mut Ui,
        tie: TieId,
        action: PointerAction,
        stage: &mut Stage<'_, CanvasCtx<'_>>,
    ) {
        let graph_ctx = stage.ctx.graph_ctx();
        match (action.edge, action.button) {
            (PointerEdge::Clicked { .. }, PointerButton::Left) => {
                let mode = ClickMode::with_shift(ui.modifiers().contains(Modifiers::SHIFT));
                stage.out.extend_graph(GraphIntent::click(
                    mode,
                    graph_ctx.selected(),
                    ItemId::Tie(tie),
                ));
            }
            (PointerEdge::Clicked { .. }, PointerButton::Right) => {
                let opened = self.tie_menu.open_on(ui, graph_ctx, tie);
                stage.out.extend_graph(opened);
            }
            (PointerEdge::DragStarted, PointerButton::Left) => {
                let start = tie_widget::grab(graph_ctx, tie);
                self.group_drag.latch(start, ui, stage.out);
            }
            _ => {}
        }
    }
}

/// A left click clears the selection, unless it is the one a double click opens the palette on.
/// Bodies hit-test first, so a click reaches bare canvas only when it missed every node.
fn deselect_on_click(action: PointerAction, stage: &mut Stage<'_, CanvasCtx<'_>>) {
    if matches!(action.edge, PointerEdge::Clicked { .. })
        && action.button == PointerButton::Left
        && stage.ctx.gesture() == Some(CanvasGesture::Deselect)
        && !stage.ctx.graph_ctx().selected().is_empty()
    {
        stage.out.push_graph(GraphIntent::clear_selection());
    }
}
