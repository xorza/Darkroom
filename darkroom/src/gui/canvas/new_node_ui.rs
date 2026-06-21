use glam::Vec2;
use palantir::{ClickOutside, Configure, MenuItem, Panel, Popup, Sizing, Spacing, Text, Ui};
use scenarium::function::FuncInput;
use scenarium::graph::{Binding, InputPort, Node};
use scenarium::prelude::NodeId;
use scenarium::subgraph::{SubgraphDef, SubgraphRef};

use crate::core::document::view_node::ViewNode;
use crate::core::edit::intent::Intent;
use crate::gui::app::AppContext;
use crate::gui::canvas::{CanvasGesture, outer_canvas_widget_id, to_world};
use crate::gui::scene::Scene;

/// A chosen palette entry: the node to spawn, an optional `Local` subgraph
/// def to add alongside it, and the default input bindings to seed with it.
type ChosenNode = (Node, Option<Box<SubgraphDef>>, Vec<(InputPort, Binding)>);

/// Right-click-on-canvas → popup that lists every `Func` in
/// `AppContext::func_lib` grouped by category. Clicking an entry emits
/// an `Intent::AddNode` placed at the click's world position (inner-
/// canvas pre-transform). Outside-click and Esc dismiss.
#[derive(Default, Debug)]
pub(crate) struct NewNodeUi {
    state: Option<OpenState>,
}

#[derive(Copy, Clone, Debug)]
struct OpenState {
    /// Inner-canvas pre-transform position derived from the click —
    /// the spawned node lands exactly under the right-clicked pixel.
    world_pos: Vec2,
    /// Surface-space anchor for [`Popup::anchored_to`].
    anchor: Vec2,
}

impl NewNodeUi {
    pub(crate) fn apply(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        gesture: Option<CanvasGesture>,
        out: &mut Vec<Intent>,
    ) {
        let resp = ui.response_for(outer_canvas_widget_id());
        if gesture == Some(CanvasGesture::NewNode)
            && let (Some(local), Some(rect)) = (resp.pointer_local, resp.rect)
        {
            self.state = Some(OpenState {
                world_pos: to_world(local, scene),
                anchor: rect.min + local,
            });
        }

        let Some(open) = self.state else {
            return;
        };

        if ui.escape_pressed() {
            self.state = None;
            return;
        }

        // Click result: the node to spawn, plus a `Local` subgraph def to
        // add alongside it (library subgraphs are localized on instance,
        // so they drop an editable copy that records its `origin`). Owned,
        // so it holds no borrow of `func_lib` past the popup body.
        let mut chosen: Option<ChosenNode> = None;
        let chrome = ui.theme.context_menu.panel.clone();
        // One column per category (an `hstack` laid left-to-right). Within
        // a column the function list is a `wrap_vstack`, so a category with
        // many functions wraps into sub-columns once it would exceed the
        // popup's height cap. The cap lives once on the popup: a bounded
        // stack constrains its children's main axis, so the popup's
        // `max_size` height flows down through the column `hstack`/`vstack`
        // into each func wrap. Everything hugs, so the popup is only as
        // wide/tall as its columns need.
        let popup_resp = Popup::anchored_to(open.anchor)
            .click_outside(ClickOutside::Dismiss)
            .background(chrome)
            .id_salt("new_node_popup")
            .size((Sizing::Hug, Sizing::Hug))
            .max_size((f32::INFINITY, ctx.theme.new_node_popup_max_height))
            .padding(Spacing::all(6.0))
            .show(ui, |ui, popup| {
                Panel::hstack()
                    .id_salt("new_node_columns")
                    .size((Sizing::Hug, Sizing::Hug))
                    .gap(12.0)
                    .show(ui, |ui| {
                        for category in sorted_categories(ctx) {
                            Panel::vstack()
                                .id_salt(("new_node_col", category))
                                .size((Sizing::Hug, Sizing::Hug))
                                .gap(4.0)
                                .show(ui, |ui| {
                                    Text::new(category.to_owned())
                                        .id_salt(("new_node_cat", category))
                                        .show(ui);
                                    Panel::wrap_vstack()
                                        .id_salt(("new_node_funcs", category))
                                        .size((Sizing::Hug, Sizing::Hug))
                                        .gap(2.0)
                                        .line_gap(12.0)
                                        .show(ui, |ui| {
                                            for func in ctx
                                                .func_lib
                                                .funcs
                                                .iter()
                                                .filter(|f| f.category == category)
                                            {
                                                if MenuItem::new(func.name.clone())
                                                    .show(ui, popup)
                                                    .clicked()
                                                {
                                                    let node: Node = func.into();
                                                    let bindings =
                                                        default_bindings(node.id, &func.inputs);
                                                    chosen = Some((node, None, bindings));
                                                }
                                            }
                                            // Shared (`Linked`) subgraph
                                            // defs of this category, after
                                            // the funcs.
                                            for def in ctx
                                                .func_lib
                                                .subgraphs
                                                .iter()
                                                .filter(|d| d.category == category)
                                            {
                                                if MenuItem::new(def.name.clone())
                                                    .show(ui, popup)
                                                    .clicked()
                                                {
                                                    // Localize on instance: drop an
                                                    // editable `Local` copy that
                                                    // records its library `origin`.
                                                    let mut local = def.fresh_copy();
                                                    local.origin = Some(def.id);
                                                    let node = Node::subgraph_instance(
                                                        &local,
                                                        SubgraphRef::Local(local.id),
                                                    );
                                                    let bindings =
                                                        default_bindings(node.id, &local.inputs);
                                                    chosen = Some((
                                                        node,
                                                        Some(Box::new(local)),
                                                        bindings,
                                                    ));
                                                }
                                            }
                                        });
                                });
                        }
                    });
            });

        if let Some((node, def, bindings)) = chosen {
            let view_node = ViewNode {
                id: node.id,
                pos: open.world_pos,
            };
            out.push(Intent::AddNode {
                view_node,
                node,
                def,
                bindings,
            });
            self.state = None;
        } else if popup_resp.dismissed || popup_resp.close_requested {
            self.state = None;
        }
    }
}

/// Every category that has a func *or* a shared subgraph def, sorted +
/// deduped — one popup column each.
fn sorted_categories<'a>(ctx: &'a AppContext<'_>) -> Vec<&'a str> {
    let mut cats: Vec<&str> = ctx
        .func_lib
        .funcs
        .iter()
        .map(|f| f.category.as_str())
        .chain(ctx.func_lib.subgraphs.iter().map(|d| d.category.as_str()))
        .collect();
    cats.sort();
    cats.dedup();
    cats
}

/// Seed each input that declares a func default with a matching
/// `Binding::Const`, so a freshly dropped node starts with its defaults
/// filled in (a sigma of `5.0`, a preset already chosen, …) instead of
/// every port unbound. Inputs without a declared default (images, required
/// custom inputs) are left for the user to wire.
fn default_bindings(node_id: NodeId, inputs: &[FuncInput]) -> Vec<(InputPort, Binding)> {
    inputs
        .iter()
        .enumerate()
        .filter_map(|(idx, input)| {
            input
                .default_value
                .clone()
                .map(|value| (InputPort::new(node_id, idx), Binding::Const(value)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use scenarium::data::{DataType, StaticValue};

    use super::*;

    fn finput(default: Option<StaticValue>) -> FuncInput {
        FuncInput {
            name: "p".to_string(),
            required: true,
            data_type: DataType::Float,
            default_value: default,
            value_variants: vec![],
        }
    }

    #[test]
    fn default_bindings_seeds_only_declared_defaults_at_original_indices() {
        let node = NodeId::unique();
        let inputs = vec![
            finput(None),                          // 0: no default → skipped
            finput(Some(StaticValue::Float(0.0))), // 1
            finput(None),                          // 2: skipped
            finput(Some(StaticValue::Float(1.0))), // 3
        ];
        assert_eq!(
            default_bindings(node, &inputs),
            vec![
                (
                    InputPort::new(node, 1),
                    Binding::Const(StaticValue::Float(0.0))
                ),
                (
                    InputPort::new(node, 3),
                    Binding::Const(StaticValue::Float(1.0))
                ),
            ]
        );
    }
}
