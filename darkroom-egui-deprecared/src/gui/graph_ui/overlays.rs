//! Overlay UI that sits on top of the graph content: top/bottom button
//! bars, the new-node popup, and the const-binding creation path that
//! the popup can route into.

use egui::{Align2, PointerButton, Pos2, Rect, Response, Sense, vec2};
use scenarium::data::StaticValue;
use scenarium::graph::Binding;

use crate::common::StableId;
use crate::gui::Gui;
use crate::gui::graph_ui::ctx::GraphContext;
use crate::gui::graph_ui::nodes::new_node::NewNodeSelection;
use crate::gui::graph_ui::port::PortKind;
use crate::gui::graph_ui::{GraphUi, ViewButtonAction};
use crate::gui::widgets::{Area, Button, Constraints, Frame};
use crate::input::InputSnapshot;
use crate::model;
use crate::model::Intent;
use crate::session::output::{FrameOutput, RunCommand};

/// Framed horizontal button row used for both overlay bars. Wraps
/// `Frame::none(id).sense(Sense::all).inner_margin(padding)` over a
/// `gui.horizontal(body)`, returning the frame's Response.
fn button_bar(gui: &mut Gui<'_>, frame_id: StableId, body: impl FnOnce(&mut Gui<'_>)) -> Response {
    let padding = gui.style.padding;
    Frame::none(frame_id)
        .sense(Sense::all())
        .inner_margin(padding)
        .show(gui, |gui| {
            gui.horizontal(body);
        })
        .response
}

impl GraphUi {
    /// Top-left bar: fit-all / view-selected / reset-view.
    pub(super) fn render_view_buttons(
        &mut self,
        gui: &mut Gui<'_>,
    ) -> (Response, Option<ViewButtonAction>) {
        let mut action: Option<ViewButtonAction> = None;

        let container = gui.container_rect();
        let rect = Rect::from_min_max(container.left_top(), container.right_bottom());
        let response = gui
            .scope(StableId::new("graph_ui_top_buttons"))
            .max_rect(rect)
            .show(|gui| {
                Constraints::new().fill_width().apply(gui);
                button_bar(gui, StableId::new("top_buttons_frame"), |gui| {
                    let btn_size = vec2(20.0, 20.0);
                    let mono_font = gui.style.mono_font.clone();

                    let response = Button::new(StableId::new("fit_all_btn"))
                        .text("a")
                        .font(mono_font.clone())
                        .size(btn_size)
                        .show(gui);
                    if response.clicked() {
                        action = Some(ViewButtonAction::FitAll);
                    }

                    let response = Button::new(StableId::new("view_selected_btn"))
                        .text("s")
                        .font(mono_font.clone())
                        .size(btn_size)
                        .show(gui);
                    if response.clicked() {
                        action = Some(ViewButtonAction::ViewSelected);
                    }

                    let response = Button::new(StableId::new("reset_view_btn"))
                        .text("r")
                        .font(mono_font)
                        .size(btn_size)
                        .show(gui);
                    if response.clicked() {
                        action = Some(ViewButtonAction::ResetView);
                    }
                })
            });

        (response, action)
    }

    /// Bottom-left bar: run-once / autorun toggle.
    pub(super) fn render_exec_buttons(
        &mut self,
        gui: &mut Gui<'_>,
        autorun: bool,
    ) -> (Response, Option<RunCommand>) {
        let mut autorun = autorun;
        let mut run_cmd: Option<RunCommand> = None;

        let response = Area::new(StableId::new("graph_ui_bottom_buttons"))
            .fixed_pos(gui.container_rect().left_bottom())
            .pivot(Align2::LEFT_BOTTOM)
            .movable(false)
            .show(gui, |gui| {
                button_bar(gui, StableId::new("bottom_buttons_frame"), |gui| {
                    let response = Button::new(StableId::new("run_btn")).text("run").show(gui);
                    if response.clicked() {
                        run_cmd = Some(RunCommand::RunOnce);
                    }

                    let response = Button::new(StableId::new("autorun_btn"))
                        .toggle(&mut autorun)
                        .text("autorun")
                        .show(gui);

                    if response.clicked() {
                        run_cmd = Some(if autorun {
                            RunCommand::StartAutorun
                        } else {
                            RunCommand::StopAutorun
                        });
                    }
                })
            })
            .inner;

        (response, run_cmd)
    }

    // ------------------------------------------------------------------------
    // New node popup
    // ------------------------------------------------------------------------

    pub(super) fn handle_new_node_popup(
        &mut self,
        gui: &mut Gui<'_>,
        input: &InputSnapshot,
        ctx: &GraphContext<'_>,
        pointer_pos: Option<Pos2>,
        background_response: &Response,
        output: &mut FrameOutput,
    ) -> bool {
        if background_response.double_clicked_by(PointerButton::Primary)
            && let Some(pos) = pointer_pos
        {
            self.new_node_ui.open(pos);
        }

        let was_open = self.new_node_ui.is_open();

        if let Some(selection) = self.new_node_ui.show(gui, input, ctx.func_lib) {
            self.handle_new_node_selection(gui, ctx, selection, output);
        } else if was_open && !self.new_node_ui.is_open() {
            self.cancel_gesture();
        }

        self.new_node_ui.is_open()
    }

    fn handle_new_node_selection(
        &mut self,
        gui: &Gui<'_>,
        ctx: &GraphContext<'_>,
        selection: NewNodeSelection,
        output: &mut FrameOutput,
    ) {
        match selection {
            NewNodeSelection::Func { func, position } => {
                let origin = gui.container_rect().min;
                let graph_pos = (position - origin - ctx.view_graph.pan) / ctx.view_graph.scale;

                // Build the new node + view-node locally; apply() inserts them.
                let node: scenarium::graph::Node = func.into();
                let view_node = model::ViewNode {
                    id: node.id,
                    pos: graph_pos.to_pos2(),
                };

                output.add_intent(Intent::AddNode { view_node, node });
            }
            NewNodeSelection::ConstBind => {
                self.create_const_binding(ctx, output);
                self.cancel_gesture();
            }
        }
    }

    fn create_const_binding(&mut self, ctx: &GraphContext<'_>, output: &mut FrameOutput) {
        let Some(connection_drag) = self.gesture.drag() else {
            return;
        };

        if connection_drag.start_port.kind != PortKind::Input {
            return;
        }

        let input_port = connection_drag.start_port;
        // Defensive: the drag's node could have vanished between start
        // and commit (undo/redo). Silently drop — nothing to bind.
        let Some(input_node) = ctx.view_graph.graph.by_id(&input_port.node_id) else {
            return;
        };
        let func_input =
            &ctx.func_lib.by_id(&input_node.func_id).unwrap().inputs[input_port.port_idx];
        let to: Binding = func_input
            .default_value
            .clone()
            .unwrap_or_else(|| StaticValue::from(&func_input.data_type))
            .into();

        output.add_intent(Intent::SetInput {
            node_id: input_port.node_id,
            input_idx: input_port.port_idx,
            to,
        });
    }
}
