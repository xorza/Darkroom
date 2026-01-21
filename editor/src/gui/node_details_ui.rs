use egui::{Color32, Pos2, Rect, Vec2};
use graph::data::DynamicValue;
use graph::graph::NodeId;
use graph::prelude::ExecutionStats;

use crate::common::TextEdit;
use crate::common::frame::Frame;
use crate::common::positioned_ui::PositionedUi;
use crate::gui::Gui;
use crate::gui::graph_ctx::GraphContext;
use crate::gui::graph_ui_interaction::GraphUiInteraction;
use crate::model::ArgumentValuesCache;
use crate::model::graph_ui_action::GraphUiAction;

const PANEL_WIDTH: f32 = 250.0;

#[derive(Debug, Default)]
pub struct NodeDetailsUi {}

impl NodeDetailsUi {
    pub fn show(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext<'_>,
        interaction: &mut GraphUiInteraction,
        argument_values_cache: &ArgumentValuesCache,
    ) {
        let Some(selected_node_id) = ctx.view_graph.selected_node_id else {
            return;
        };

        let graph_rect = gui.rect;
        let padding = gui.style.padding;
        let panel_rect = Rect::from_min_size(
            Pos2::new(graph_rect.right() - PANEL_WIDTH, graph_rect.top() + padding),
            Vec2::new(PANEL_WIDTH - padding, graph_rect.height() - padding * 2.0),
        );

        let popup_id = gui.ui().make_persistent_id("node_details_panel");

        PositionedUi::new(popup_id, panel_rect.min)
            .rect(panel_rect)
            .max_size(panel_rect.size())
            .interactable(false)
            .show(gui, |gui| {
                Frame::popup(&gui.style.popup)
                    .inner_margin(gui.style.padding)
                    .show(gui, |gui| {
                        self.show_content(
                            gui,
                            ctx,
                            selected_node_id,
                            interaction,
                            argument_values_cache,
                        );
                    });
            });
    }

    fn show_content(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext<'_>,
        node_id: NodeId,
        interaction: &mut GraphUiInteraction,
        argument_values_cache: &ArgumentValuesCache,
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
                .char_limit(20)
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

        // Display execution info
        if let Some(stats) = ctx.execution_stats {
            self.show_execution_info(gui, node_id, stats);
        }

        // Request argument values if not cached
        if argument_values_cache.get(&node_id).is_none() {
            interaction.request_argument_values = Some(node_id);
        }

        // Display cached argument values
        if let Some(values) = argument_values_cache.get(&node_id) {
            gui.ui().add_space(8.0);
            gui.ui().separator();
            gui.ui().add_space(4.0);

            // Get function info for input/output names
            let node = ctx.view_graph.graph.by_id(&node_id).unwrap();
            let func = ctx.func_lib.by_id(&node.func_id);

            // Display inputs
            if !values.inputs.is_empty() {
                gui.ui().label("Inputs:");
                for (idx, input_value) in values.inputs.iter().enumerate() {
                    let input_name = func
                        .and_then(|f| f.inputs.get(idx))
                        .map(|i| i.name.as_str())
                        .unwrap_or("?");
                    let value_str = match input_value {
                        Some(v) => format_dynamic_value(v),
                        None => "-".to_string(),
                    };
                    gui.ui().label(format!("  {input_name}: {value_str}"));
                }
            }

            // Display outputs
            if !values.outputs.is_empty() {
                gui.ui().add_space(4.0);
                gui.ui().label("Outputs:");
                for (idx, output_value) in values.outputs.iter().enumerate() {
                    let output_name = func
                        .and_then(|f| f.outputs.get(idx))
                        .map(|o| o.name.as_str())
                        .unwrap_or("?");
                    let value_str = format_dynamic_value(output_value);
                    gui.ui().label(format!("  {output_name}: {value_str}"));
                }
            }
        }
    }

    fn show_execution_info(&self, gui: &mut Gui<'_>, node_id: NodeId, stats: &ExecutionStats) {
        gui.ui().add_space(8.0);
        gui.ui().separator();
        gui.ui().add_space(4.0);
        gui.ui().label("Execution:");

        // Check for error
        if let Some(node_error) = stats.node_errors.iter().find(|e| e.node_id == node_id) {
            let error_color = Color32::from_rgb(255, 100, 100);
            gui.ui()
                .colored_label(error_color, format!("  Error: {}", node_error.error));
            return;
        }

        // Check if cached
        if stats.cached_nodes.contains(&node_id) {
            gui.ui().label("  Status: cached");
            return;
        }

        // Check for missing inputs
        if stats.missing_inputs.iter().any(|p| p.target_id == node_id) {
            let warning_color = Color32::from_rgb(255, 180, 70);
            gui.ui()
                .colored_label(warning_color, "  Status: missing inputs");
            return;
        }

        // Check if executed and show time
        if let Some(executed) = stats.executed_nodes.iter().find(|e| e.node_id == node_id) {
            gui.ui()
                .label(format!("  Time: {:.2} ms", executed.elapsed_secs * 1000.0));
        } else {
            gui.ui().label("  Status: not executed");
        }
    }
}

fn format_dynamic_value(value: &DynamicValue) -> String {
    match value {
        DynamicValue::None => "-".to_string(),
        DynamicValue::Null => "null".to_string(),
        DynamicValue::Float(f) => format!("{f:.4}"),
        DynamicValue::Int(i) => i.to_string(),
        DynamicValue::Bool(b) => b.to_string(),
        DynamicValue::String(s) => format!("\"{s}\""),
        DynamicValue::FsPath(s) => format!("\"{s}\""),
        DynamicValue::Custom { type_id, .. } => format!("<{type_id:?}>"),
        DynamicValue::Enum { variant_name, .. } => variant_name.clone(),
    }
}
