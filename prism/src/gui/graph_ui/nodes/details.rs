use egui::{Pos2, Rect, Response, Sense, Vec2};
use palantir::Image;
use scenarium::data::DynamicValue;
use scenarium::graph::NodeId;
use scenarium::prelude::Func;

use crate::common::StableId;
use crate::gui::Gui;
use crate::gui::graph_ui::ctx::GraphContext;
use crate::gui::graph_ui::frame_output::FrameOutput;
use crate::gui::image_utils::to_color_image;
use crate::gui::widgets::{
    Frame, HitRegion, Image as ImageWidget, Label, PositionedUi, ScrollArea, Separator, Space,
    TextEdit, Texture,
};
use crate::model::argument_values_cache::{ArgumentValuesCache, CachedTexture, NodeCache};
use crate::model::graph_ui_action::GraphUiAction;
use crate::model::node_execution::NodeExecutionInfo;

const PANEL_WIDTH: f32 = 250.0;
const PREVIEW_MAX_WIDTH: f32 = PANEL_WIDTH - 32.0;

#[derive(Debug, Default)]
pub struct NodeDetailsUi;

impl NodeDetailsUi {
    pub fn show(
        &self,
        gui: &mut Gui<'_>,
        ctx: &GraphContext<'_>,
        cache: &mut ArgumentValuesCache,
        output: &mut FrameOutput,
    ) -> Response {
        let panel_rect = compute_panel_rect(gui);
        let popup_id = StableId::new("node_details_panel");

        let Some(node_id) = ctx.view_graph.selected_node_id else {
            return HitRegion::new(popup_id).sense(Sense::empty()).show(gui);
        };

        let scroll_id = StableId::new("node_details_scroll");

        PositionedUi::new(popup_id, panel_rect.min)
            .rect(panel_rect)
            .max_size(panel_rect.size())
            .show(gui, |gui| {
                Frame::popup(StableId::new("node_details_frame"), &gui.style.popup)
                    .inner_margin(gui.style.padding)
                    .sense(Sense::all())
                    .show(gui, |gui| {
                        ScrollArea::vertical(scroll_id).show(gui, |gui| {
                            show_content(gui, ctx, cache, node_id, output);
                        });
                    })
            })
            .inner
            .response
    }
}

fn compute_panel_rect(gui: &Gui<'_>) -> Rect {
    let graph_rect = gui.rect;
    let padding = gui.style.padding;
    Rect::from_min_size(
        Pos2::new(graph_rect.right() - PANEL_WIDTH, graph_rect.top() + padding),
        Vec2::new(PANEL_WIDTH - padding, graph_rect.height() - padding * 2.0),
    )
}

fn show_content(
    gui: &mut Gui<'_>,
    ctx: &GraphContext<'_>,
    cache: &mut ArgumentValuesCache,
    node_id: NodeId,
    output: &mut FrameOutput,
) {
    // `selected_node_id` is cleared by `remove_node`, so this lookup
    // should always succeed — but an undo/redo between frames can
    // invalidate the id before we land here.
    let Some(node) = ctx.view_graph.graph.by_id(&node_id) else {
        return;
    };
    let original_name = node.name.clone();

    show_name_editor(gui, node_id, &original_name, output);

    if ctx.execution_stats.is_some() {
        show_execution_info(gui, ctx, node_id);
    }

    let Some(func) = ctx.func_lib.by_id(&node.func_id) else {
        return;
    };

    let Some(node_cache) = cache.get_mut(&node_id) else {
        // Gate duplicate requests at the source: only emit when this
        // node isn't already pending or ready. Session::handle_output
        // relays the request unconditionally — dedupe is the cache's
        // job, and the cache lives here.
        if cache.mark_pending(node_id) {
            output.set_request_argument_values(node_id);
        }
        return;
    };

    add_section_separator(gui);
    show_image_previews(gui, node_id, func, node_cache);
}

fn show_name_editor(
    gui: &mut Gui<'_>,
    node_id: NodeId,
    original_name: &str,
    output: &mut FrameOutput,
) {
    let mut name = original_name.to_string();

    gui.vertical(|gui| {
        let font = gui.style.sub_font.clone();
        let text_color = gui.style.text_color;

        Label::new("Name:").show(gui);
        // Stable salt (not keyed on node_id) — the textbox is the
        // same chrome widget regardless of which node is selected;
        // only its content changes. Keying on node_id would shift
        // the widget id on every selection change, tripping egui's
        // "widget rect changed id between passes" warning.
        TextEdit::singleline(&mut name)
            .id_salt(StableId::new("node_name_edit"))
            .font(font)
            .text_color(text_color)
            .char_limit(20)
            .show(gui);
    });

    if name != original_name {
        // Mutation applied via NodeNameChanged::apply in commit_actions.
        output.add_action(GraphUiAction::NodeNameChanged {
            node_id,
            before: original_name.to_string(),
            after: name,
        });
    }
}

// === Execution Info ===

fn show_execution_info(gui: &mut Gui<'_>, ctx: &GraphContext<'_>, node_id: NodeId) {
    add_section_separator(gui);
    Label::new("Execution:").show(gui);

    let info = ctx.exec_info_index.get(node_id);
    match info {
        NodeExecutionInfo::Errored(node_error) => {
            let func_name = match &node_error.error {
                scenarium::execution_graph::Error::Invoke { func_id, .. } => ctx
                    .func_lib
                    .by_id(func_id)
                    .map_or_else(|| format!("<unknown func {func_id}>"), |f| f.name.clone()),
                scenarium::execution_graph::Error::CycleDetected { .. } => "cycle".to_string(),
            };
            let color = gui.style.node.errored_shadow.color;
            Label::new(format!("  {func_name}: {}", node_error.error))
                .color(color)
                .show(gui);
        }
        NodeExecutionInfo::Cached => {
            Label::new("  Status: cached").show(gui);
        }
        NodeExecutionInfo::MissingInputs => {
            let color = gui.style.node.missing_inputs_shadow.color;
            Label::new("  Status: missing inputs")
                .color(color)
                .show(gui);
        }
        NodeExecutionInfo::Executed(executed) => {
            let elapsed_ms = executed.elapsed_secs * 1000.0;
            Label::new(format!("  Time: {elapsed_ms:.2} ms")).show(gui);
        }
        NodeExecutionInfo::None => {
            Label::new("  Status: not executed").show(gui);
        }
    }
}

// === Image Previews ===

fn show_image_previews(
    gui: &mut Gui<'_>,
    node_id: NodeId,
    func: &Func,
    node_cache: &mut NodeCache,
) {
    cache_previews(
        gui,
        &node_id,
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
        &node_id,
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
        Space::new(gui.style.small_padding).show(gui);
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

    Label::new(label).show(gui);
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
        let texture_handle = Texture::new(
            format!("node_preview_{node_id}_{prefix}_{idx}"),
            to_color_image(preview),
        )
        .load(gui);

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
    Label::new(format!("  {name}: {value_str}")).show(gui);

    let Some(texture) = texture else {
        return;
    };

    let aspect = texture.desc.width as f32 / texture.desc.height as f32;
    let display_width = PREVIEW_MAX_WIDTH.min(texture.desc.width as f32);
    let display_height = display_width / aspect;

    Space::new(gui.style.small_padding).show(gui);
    ImageWidget::new(
        texture.handle.id(),
        Vec2::new(display_width, display_height),
    )
    .show(gui);
}

// === Helpers ===

fn add_section_separator(gui: &mut Gui<'_>) {
    Space::new(gui.style.padding).show(gui);
    Separator::new().show(gui);
    Space::new(gui.style.small_padding).show(gui);
}
