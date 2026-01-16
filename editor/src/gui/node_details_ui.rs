use egui::{Align2, Order, Pos2, Rect, Vec2};
use graph::graph::NodeId;

use crate::common::TextEdit;
use crate::common::area::Area;
use crate::common::frame::Frame;
use crate::gui::Gui;
use crate::gui::graph_ctx::GraphContext;
use crate::gui::graph_ui_interaction::{GraphUiAction, GraphUiInteraction};

const PANEL_WIDTH: f32 = 250.0;

#[derive(Debug, Default)]
pub struct NodeDetailsUi {}

impl NodeDetailsUi {
    pub fn show(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext<'_>,
        graph_rect: Rect,
        interaction: &mut GraphUiInteraction,
    ) {
        let Some(selected_node_id) = ctx.view_graph.selected_node_id else {
            return;
        };

        let panel_rect = Rect::from_min_size(
            Pos2::new(graph_rect.right() - PANEL_WIDTH, graph_rect.top()),
            Vec2::new(PANEL_WIDTH, graph_rect.height()),
        );

        let popup_id = gui.ui().make_persistent_id("node_details_panel");

        Area::new(popup_id)
            .fixed_pos(panel_rect.min)
            .order(Order::Foreground)
            .movable(false)
            .interactable(true)
            .show(gui, |gui| {
                Frame::popup(&gui.style.popup).show(gui, |gui| {
                    let padding = gui.style.padding;
                    gui.ui().set_min_width(panel_rect.width() - padding * 2.0);
                    gui.ui().set_min_height(panel_rect.height() - padding * 2.0);

                    self.show_content(gui, ctx, selected_node_id, interaction);
                });
            });
    }

    fn show_content(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext<'_>,
        node_id: NodeId,
        interaction: &mut GraphUiInteraction,
    ) {
        // Get current name from node
        let original_name = ctx.view_graph.graph.by_id(&node_id).unwrap().name.clone();
        let mut name = original_name.clone();

        gui.vertical(|gui| {
            let font = gui.style.sub_font.clone();
            let text_color = gui.style.text_color;

            // Node name label
            gui.ui().label("Name:");

            // Node name text edit
            TextEdit::singleline(&mut name)
                .font(font)
                .text_color(text_color)
                .show(gui);
        });

        // Update node name if changed
        if name != original_name {
            let node = ctx.view_graph.graph.by_id_mut(&node_id).unwrap();
            node.name = name.clone();

            interaction.add_action(GraphUiAction::NodeNameChanged {
                node_id,
                before: original_name,
                after: name,
            });
        }
    }
}
