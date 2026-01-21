use egui::{Color32, Pos2, Rect, TextureOptions, Vec2};
use graph::data::DynamicValue;
use graph::graph::{Node, NodeId};
use graph::prelude::{ExecutionStats, Func};
use vision::Image;

use crate::common::TextEdit;
use crate::common::frame::Frame;
use crate::common::image_utils::to_color_image;
use crate::common::positioned_ui::PositionedUi;
use crate::gui::Gui;
use crate::gui::graph_ctx::GraphContext;
use crate::gui::graph_ui_interaction::GraphUiInteraction;
use crate::model::ArgumentValuesCache;
use crate::model::argument_values_cache::{CachedTexture, NodeCache};
use crate::model::graph_ui_action::GraphUiAction;

const PANEL_WIDTH: f32 = 250.0;
const PREVIEW_MAX_WIDTH: f32 = PANEL_WIDTH - 32.0;

#[derive(Debug, Default)]
pub struct NodeDetailsUi {}

impl NodeDetailsUi {
    pub fn show(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext<'_>,
        interaction: &mut GraphUiInteraction,
        argument_values_cache: &mut ArgumentValuesCache,
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
        argument_values_cache: &mut ArgumentValuesCache,
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
            self.show_execution_info(gui, ctx, node_id, stats);
        }

        // Display cached argument values
        let Some(node_cache) = argument_values_cache.get_mut(&node_id) else {
            interaction.request_argument_values = Some(node_id);
            return;
        };

        gui.ui().add_space(8.0);
        gui.ui().separator();
        gui.ui().add_space(4.0);

        // Get function info for input/output names
        let node = ctx.view_graph.graph.by_id(&node_id).unwrap();
        let func = ctx.func_lib.by_id(&node.func_id).unwrap();

        Self::show_image_previews(gui, node, func, node_cache);
    }

    fn show_image_previews(
        gui: &mut Gui<'_>,
        node: &Node,
        func: &Func,
        node_cache: &mut NodeCache,
    ) {
        for (idx, input) in node_cache.arg_values.inputs.iter().enumerate() {
            if let Some(value) = input.as_ref()
                && let Some(image) = value.as_custom::<Image>()
                && let Some(preview) = image.take_preview()
            {
                let desc = *preview.desc();
                let color_image = to_color_image(preview);
                let texture_handle = gui.ui().ctx().load_texture(
                    format!("node_preview_{}_input_{idx}", node.id),
                    color_image,
                    TextureOptions::LINEAR,
                );
                let cached_texture = CachedTexture {
                    desc,
                    handle: texture_handle,
                };

                if idx >= node_cache.input_previews.len() {
                    node_cache.input_previews.resize(idx + 1, None);
                }

                node_cache.input_previews[idx] = Some(cached_texture);
            }
        }

        for (idx, value) in node_cache.arg_values.outputs.iter().enumerate() {
            if let Some(image) = value.as_custom::<Image>()
                && let Some(preview) = image.take_preview()
            {
                let desc = *preview.desc();
                let color_image = to_color_image(preview);
                let texture_handle = gui.ui().ctx().load_texture(
                    format!("node_preview_{}_output_{idx}", node.id),
                    color_image,
                    TextureOptions::LINEAR,
                );

                let cached_texture = CachedTexture {
                    desc,
                    handle: texture_handle,
                };

                if idx >= node_cache.output_previews.len() {
                    node_cache.output_previews.resize(idx + 1, None);
                }

                node_cache.output_previews[idx] = Some(cached_texture);
            }
        }

        // Display inputs
        if !node_cache.arg_values.inputs.is_empty() {
            gui.ui().label("Inputs:");
            for (idx, input_value) in node_cache.arg_values.inputs.iter().enumerate() {
                let input_name = func.inputs[idx].name.as_str();
                let value_str = match input_value {
                    Some(v) => v.to_string(),
                    None => "-".to_string(),
                };
                gui.ui().label(format!("  {input_name}: {value_str}"));

                if let Some(Some(texture)) = node_cache.input_previews.get(idx) {
                    // Calculate display size maintaining aspect ratio
                    let aspect = texture.desc.width as f32 / texture.desc.height as f32;
                    let display_width = PREVIEW_MAX_WIDTH.min(texture.desc.width as f32);
                    let display_height = display_width / aspect;

                    gui.ui().add_space(4.0);
                    gui.ui().image((
                        texture.handle.id(),
                        Vec2::new(display_width, display_height),
                    ));
                }
            }
        }

        // Display outputs
        if !node_cache.arg_values.outputs.is_empty() {
            gui.ui().add_space(4.0);
            gui.ui().label("Outputs:");
            for (idx, output_value) in node_cache.arg_values.outputs.iter().enumerate() {
                let output_name = func.outputs[idx].name.as_str();
                let value_str = output_value.to_string();
                gui.ui().label(format!("  {output_name}: {value_str}"));

                if let Some(Some(texture)) = node_cache.output_previews.get(idx) {
                    // Calculate display size maintaining aspect ratio
                    let aspect = texture.desc.width as f32 / texture.desc.height as f32;
                    let display_width = PREVIEW_MAX_WIDTH.min(texture.desc.width as f32);
                    let display_height = display_width / aspect;

                    gui.ui().add_space(4.0);
                    gui.ui().image((
                        texture.handle.id(),
                        Vec2::new(display_width, display_height),
                    ));
                }
            }
        }
    }

    fn show_execution_info(
        &self,
        gui: &mut Gui<'_>,
        ctx: &GraphContext<'_>,
        node_id: NodeId,
        stats: &ExecutionStats,
    ) {
        gui.ui().add_space(8.0);
        gui.ui().separator();
        gui.ui().add_space(4.0);
        gui.ui().label("Execution:");

        // Check for error
        if let Some(node_error) = stats.node_errors.iter().find(|e| e.node_id == node_id) {
            let error_color = Color32::from_rgb(255, 100, 100);
            let func_name = match &node_error.error {
                graph::execution_graph::Error::Invoke { func_id, .. } => ctx
                    .func_lib
                    .by_id(func_id)
                    .map(|f| f.name.as_str())
                    .unwrap(),
                graph::execution_graph::Error::CycleDetected { .. } => "cycle",
            };
            gui.ui().colored_label(
                error_color,
                format!("  {}: {}", func_name, node_error.error),
            );
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
