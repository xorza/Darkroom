use egui::{Color32, Pos2, Rect, Response, Sense, TextureOptions, Vec2};
use palantir::Image;
use scenarium::data::DynamicValue;
use scenarium::graph::{Node, NodeId};
use scenarium::prelude::{ExecutionStats, Func};

use crate::common::TextEdit;
use crate::common::frame::Frame;
use crate::common::image_utils::to_color_image;
use crate::common::positioned_ui::PositionedUi;
use crate::common::scroll_area::ScrollArea;
use crate::gui::Gui;
use crate::gui::graph_ctx::GraphContext;
use crate::gui::graph_ui_interaction::GraphUiInteraction;
use crate::gui::node_ui::NodeExecutionInfo;
use crate::model::argument_values_cache::{CachedTexture, NodeCache};
use crate::model::graph_ui_action::GraphUiAction;

const PANEL_WIDTH: f32 = 250.0;
const PREVIEW_MAX_WIDTH: f32 = PANEL_WIDTH - 32.0;

#[derive(Debug, Default)]
pub struct NodeDetailsUi;

impl NodeDetailsUi {
    pub fn show(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext<'_>,
        interaction: &mut GraphUiInteraction,
    ) -> Response {
        let panel_rect = self.compute_panel_rect(gui);
        let popup_id = gui.ui().make_persistent_id("node_details_panel");

        let Some(node_id) = ctx.view_graph.selected_node_id else {
            return gui.ui().interact(Rect::NOTHING, popup_id, Sense::empty());
        };

        let scroll_id = gui.ui().make_persistent_id("node_details_scroll");

        PositionedUi::new(popup_id, panel_rect.min)
            .rect(panel_rect)
            .max_size(panel_rect.size())
            .show(gui, |gui| {
                Frame::popup(&gui.style.popup)
                    .inner_margin(gui.style.padding)
                    .sense(Sense::all())
                    .show(gui, |gui| {
                        ScrollArea::vertical().id(scroll_id).show(gui, |gui| {
                            self.show_content(gui, ctx, node_id, interaction);
                        });
                    })
            })
            .inner
            .response
    }

    fn compute_panel_rect(&self, gui: &Gui<'_>) -> Rect {
        let graph_rect = gui.rect;
        let padding = gui.style.padding;
        Rect::from_min_size(
            Pos2::new(graph_rect.right() - PANEL_WIDTH, graph_rect.top() + padding),
            Vec2::new(PANEL_WIDTH - padding, graph_rect.height() - padding * 2.0),
        )
    }

    fn show_content(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext<'_>,
        node_id: NodeId,
        interaction: &mut GraphUiInteraction,
    ) {
        self.show_name_editor(gui, ctx, node_id, interaction);

        if let Some(stats) = ctx.execution_stats {
            show_execution_info(gui, ctx, node_id, stats);
        }

        let Some(node_cache) = ctx.argument_values_cache.get_mut(&node_id) else {
            interaction.request_argument_values = Some(node_id);
            return;
        };

        add_section_separator(gui);

        let node = ctx.view_graph.graph.by_id(&node_id).unwrap();
        let func = ctx.func_lib.by_id(&node.func_id).unwrap();
        show_image_previews(gui, node, func, node_cache);
    }

    fn show_name_editor(
        &self,
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext<'_>,
        node_id: NodeId,
        interaction: &mut GraphUiInteraction,
    ) {
        let original_name = ctx.view_graph.graph.by_id(&node_id).unwrap().name.clone();
        let mut name = original_name.clone();

        gui.vertical(|gui| {
            let font = gui.style.sub_font.clone();
            let text_color = gui.style.text_color;

            gui.ui().label("Name:");
            TextEdit::singleline(&mut name)
                .font(font)
                .text_color(text_color)
                .char_limit(20)
                .show(gui);
        });

        if name != original_name {
            ctx.view_graph
                .graph
                .by_id_mut(&node_id)
                .unwrap()
                .name
                .clone_from(&name);
            interaction.add_action(GraphUiAction::NodeNameChanged {
                node_id,
                before: original_name,
                after: name,
            });
        }
    }
}

// === Execution Info ===

fn show_execution_info(
    gui: &mut Gui<'_>,
    ctx: &GraphContext<'_>,
    node_id: NodeId,
    stats: &ExecutionStats,
) {
    add_section_separator(gui);
    gui.ui().label("Execution:");

    let info = NodeExecutionInfo::from_stats(Some(stats), node_id);
    match info {
        NodeExecutionInfo::Errored(node_error) => {
            let func_name = match &node_error.error {
                scenarium::execution_graph::Error::Invoke { func_id, .. } => {
                    ctx.func_lib.by_id(func_id).unwrap().name.clone()
                }
                scenarium::execution_graph::Error::CycleDetected { .. } => "cycle".to_string(),
            };
            gui.ui().colored_label(
                Color32::from_rgb(255, 100, 100),
                format!("  {func_name}: {}", node_error.error),
            );
        }
        NodeExecutionInfo::Cached => {
            gui.ui().label("  Status: cached");
        }
        NodeExecutionInfo::MissingInputs => {
            gui.ui()
                .colored_label(Color32::from_rgb(255, 180, 70), "  Status: missing inputs");
        }
        NodeExecutionInfo::Executed(executed) => {
            let elapsed_ms = executed.elapsed_secs * 1000.0;
            gui.ui().label(format!("  Time: {elapsed_ms:.2} ms"));
        }
        NodeExecutionInfo::None => {
            gui.ui().label("  Status: not executed");
        }
    }
}

// === Image Previews ===

fn show_image_previews(gui: &mut Gui<'_>, node: &Node, func: &Func, node_cache: &mut NodeCache) {
    cache_previews(
        gui,
        &node.id,
        "input",
        node_cache
            .arg_values
            .inputs
            .iter()
            .map(|v| v.as_ref().map(|v| v as &DynamicValue)),
        &mut node_cache.input_previews,
    );
    cache_previews(
        gui,
        &node.id,
        "output",
        node_cache.arg_values.outputs.iter().map(Some),
        &mut node_cache.output_previews,
    );

    show_values_section(
        gui,
        "Inputs:",
        &node_cache.arg_values.inputs,
        &func.inputs,
        &node_cache.input_previews,
        |v| v.as_ref().map_or("-".to_string(), |v| v.to_string()),
        |p| p.name.as_str(),
    );

    if !node_cache.arg_values.outputs.is_empty() {
        gui.ui().add_space(4.0);
    }

    show_values_section(
        gui,
        "Outputs:",
        &node_cache.arg_values.outputs,
        &func.outputs,
        &node_cache.output_previews,
        |v| v.to_string(),
        |p| p.name.as_str(),
    );
}

fn show_values_section<T, P>(
    gui: &mut Gui<'_>,
    label: &str,
    values: &[T],
    ports: &[P],
    previews: &[Option<CachedTexture>],
    format_value: impl Fn(&T) -> String,
    get_name: impl Fn(&P) -> &str,
) {
    if values.is_empty() {
        return;
    }

    gui.ui().label(label);
    for (idx, value) in values.iter().enumerate() {
        let name = get_name(&ports[idx]);
        let value_str = format_value(value);
        show_value_with_preview(
            gui,
            name,
            &value_str,
            previews.get(idx).and_then(|t| t.as_ref()),
        );
    }
}

fn cache_previews<'a>(
    gui: &mut Gui<'_>,
    node_id: &NodeId,
    prefix: &str,
    values: impl Iterator<Item = Option<&'a DynamicValue>>,
    cache: &mut Vec<Option<CachedTexture>>,
) {
    for (idx, value) in values.enumerate() {
        let Some(image) = value.and_then(|v| v.as_custom::<Image>()) else {
            continue;
        };
        let Some(preview) = image.take_preview() else {
            continue;
        };

        let desc = *preview.desc();
        let texture_handle = gui.ui().ctx().load_texture(
            format!("node_preview_{node_id}_{prefix}_{idx}"),
            to_color_image(preview),
            TextureOptions::LINEAR,
        );

        if idx >= cache.len() {
            cache.resize(idx + 1, None);
        }
        cache[idx] = Some(CachedTexture {
            desc,
            handle: texture_handle,
        });
    }
}

fn show_value_with_preview(
    gui: &mut Gui<'_>,
    name: &str,
    value_str: &str,
    texture: Option<&CachedTexture>,
) {
    gui.ui().label(format!("  {name}: {value_str}"));

    let Some(texture) = texture else {
        return;
    };

    let aspect = texture.desc.width as f32 / texture.desc.height as f32;
    let display_width = PREVIEW_MAX_WIDTH.min(texture.desc.width as f32);
    let display_height = display_width / aspect;

    gui.ui().add_space(4.0);
    gui.ui().image((
        texture.handle.id(),
        Vec2::new(display_width, display_height),
    ));
}

// === Helpers ===

fn add_section_separator(gui: &mut Gui<'_>) {
    gui.ui().add_space(8.0);
    gui.ui().separator();
    gui.ui().add_space(4.0);
}
