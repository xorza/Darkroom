//! Overlay UI that sits on top of the graph content: top/bottom button
//! bars, the new-node popup, and the const-binding creation path that
//! the popup can route into.

use bumpalo::Bump;
use egui::{Align2, Id, PointerButton, Pos2, Response, Sense, pos2, vec2};
use scenarium::data::StaticValue;
use scenarium::graph::Binding;

use crate::common::StableId;
use crate::common::button::Button;
use crate::common::frame::Frame;
use crate::common::positioned_ui::PositionedUi;
use crate::gui::Gui;
use crate::gui::connection_ui::PortKind;
use crate::gui::frame_output::RunCommand;
use crate::gui::graph_ctx::GraphContext;
use crate::gui::graph_ui::{ButtonResult, GraphUi, ViewButtonAction};
use crate::gui::new_node_ui::NewNodeSelection;
use crate::input::InputSnapshot;
use crate::model;
use crate::model::graph_ui_action::GraphUiAction;

impl GraphUi {
    pub(super) fn render_buttons(&mut self, gui: &mut Gui<'_>, autorun: bool) -> ButtonResult {
        let mut autorun = autorun;
        let rect = gui.rect;
        let mut action: Option<ViewButtonAction> = None;

        // Top buttons (view controls)
        let mut response =
            PositionedUi::new(StableId::new("graph_ui_top_buttons"), rect.left_top())
                .pivot(Align2::LEFT_TOP)
                .interactable(false)
                .show(gui, |gui| {
                    gui.ui().take_available_width();
                    let padding = gui.style.padding;

                    Frame::none()
                        .sense(Sense::all())
                        .inner_margin(padding)
                        .show(gui, StableId::new("top_buttons_frame"), |gui| {
                            gui.horizontal(|gui| {
                                let btn_size = vec2(20.0, 20.0);
                                let mono_font = gui.style.mono_font.clone();

                                let response = Button::default()
                                    .text("a")
                                    .font(mono_font.clone())
                                    .size(btn_size)
                                    .show(gui, StableId::new("fit_all_btn"));
                                if response.clicked() {
                                    action = Some(ViewButtonAction::FitAll);
                                }

                                let response = Button::default()
                                    .text("s")
                                    .font(mono_font.clone())
                                    .size(btn_size)
                                    .show(gui, StableId::new("view_selected_btn"));
                                if response.clicked() {
                                    action = Some(ViewButtonAction::ViewSelected);
                                }

                                let response = Button::default()
                                    .text("r")
                                    .font(mono_font)
                                    .size(btn_size)
                                    .show(gui, StableId::new("reset_view_btn"));
                                if response.clicked() {
                                    action = Some(ViewButtonAction::ResetView);
                                }
                            });
                        })
                        .response
                })
                .inner;

        // Bottom buttons (execution controls)
        response |= PositionedUi::new(
            StableId::new("graph_ui_bottom_buttons"),
            pos2(rect.left(), rect.bottom()),
        )
        .pivot(Align2::LEFT_BOTTOM)
        .interactable(false)
        .show(gui, |gui| {
            let padding = gui.style.padding;

            Frame::none()
                .sense(Sense::all())
                .inner_margin(padding)
                .show(gui, StableId::new("bottom_buttons_frame"), |gui| {
                    gui.horizontal(|gui| {
                        let response = Button::default()
                            .text("run")
                            .show(gui, StableId::new("run_btn"));
                        if response.clicked() {
                            self.output.set_run_cmd(RunCommand::RunOnce);
                        }

                        let response = Button::default()
                            .toggle(&mut autorun)
                            .text("autorun")
                            .show(gui, StableId::new("autorun_btn"));

                        if response.clicked() {
                            self.output.set_run_cmd(if autorun {
                                RunCommand::StartAutorun
                            } else {
                                RunCommand::StopAutorun
                            });
                        }
                    });
                })
                .response
        })
        .inner;

        ButtonResult { response, action }
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
        arena: &Bump,
    ) -> bool {
        if background_response.double_clicked_by(PointerButton::Primary)
            && let Some(pos) = pointer_pos
        {
            self.new_node_ui.open(pos);
        }

        let was_open = self.new_node_ui.is_open();

        if let Some(selection) = self.new_node_ui.show(gui, input, ctx.func_lib, arena) {
            self.handle_new_node_selection(gui, ctx, selection);
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
    ) {
        match selection {
            NewNodeSelection::Func(func) => {
                let screen_pos = self.new_node_ui.position();
                let origin = gui.rect.min;
                let graph_pos = (screen_pos - origin - ctx.view_graph.pan) / ctx.view_graph.scale;

                // Build the new node + view-node locally; apply() inserts them.
                let node: scenarium::graph::Node = func.into();
                let view_node = model::ViewNode {
                    id: node.id,
                    pos: graph_pos.to_pos2(),
                };

                self.output
                    .add_action(GraphUiAction::NodeAdded { view_node, node });
            }
            NewNodeSelection::ConstBind => {
                self.create_const_binding(ctx);
                self.cancel_gesture();
            }
        }
    }

    fn create_const_binding(&mut self, ctx: &GraphContext<'_>) {
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
        let before = input_node.inputs[input_port.port_idx].binding.clone();
        let after: Binding = func_input
            .default_value
            .clone()
            .unwrap_or_else(|| StaticValue::from(&func_input.data_type))
            .into();

        self.output.add_action(GraphUiAction::InputChanged {
            node_id: input_port.node_id,
            input_idx: input_port.port_idx,
            before,
            after,
        });
    }
}
