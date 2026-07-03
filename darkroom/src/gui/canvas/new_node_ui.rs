use glam::Vec2;
use palantir::{Configure, MenuItem, Panel, PopupHandle, Sizing, Text, Ui};
use scenarium::graph::subgraph::{SubgraphDef, SubgraphRef};
use scenarium::graph::{Binding, InputPort, Node, NodeKind};
use scenarium::node::function::FuncInput;
use scenarium::node::special::{ALL as SPECIAL_NODES, SpecialNode};
use scenarium::prelude::NodeId;

use crate::core::document::view_node::ViewNode;
use crate::core::edit::intent::Intent;
use crate::gui::PortRef;
use crate::gui::app::AppContext;
use crate::gui::canvas::anchored_menu::AnchoredMenu;
use crate::gui::canvas::{CanvasGesture, outer_canvas_widget_id, to_world};
use crate::gui::scene::Scene;

/// A chosen palette entry: the node to spawn, an optional `Local` subgraph
/// def to add alongside it, and the default input bindings to seed with it.
#[derive(Debug)]
struct ChosenNode {
    node: Node,
    def: Option<Box<SubgraphDef>>,
    bindings: Vec<(InputPort, Binding)>,
}

/// Right-click or double-click on empty canvas → popup that lists every
/// `Func` in `AppContext::library` grouped by category. Clicking an entry
/// emits an `Intent::AddNode` placed at the click's world position (inner-
/// canvas pre-transform). Outside-click and Esc dismiss.
#[derive(Default, Debug)]
pub(crate) struct NewNodeUi {
    menu: AnchoredMenu,
    /// Inner-canvas pre-transform position of the current open — the
    /// spawned node lands exactly under the click. Set at open, read at pick.
    world_pos: Vec2,
    /// Source port when a connection dropped on empty canvas opened the
    /// palette; on pick the wire resumes floating from it (rather than
    /// auto-attaching). `None` for a plain RMB / double-click. Set at open,
    /// read at pick.
    source: Option<PortRef>,
    /// Set when a node was picked from a popup that a dropped connection
    /// opened: the wire's source port, handed back to `ConnectionUI` so the
    /// wire resumes *floating* and the user clicks the exact port to land
    /// it. Taken by the canvas next frame.
    resume_floating: Option<PortRef>,
}

impl NewNodeUi {
    pub(crate) fn apply(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        gesture: Option<CanvasGesture>,
        pending_source: Option<PortRef>,
        out: &mut Vec<Intent>,
    ) {
        let resp = ui.response_for(outer_canvas_widget_id());
        // Open the palette either from a bare RMB / double-click (`NewNode`
        // gesture) or from a connection dropped on empty canvas
        // (`pending_source`). Placement is the same — under the pointer.
        if (pending_source.is_some() || gesture == Some(CanvasGesture::NewNode))
            && let (Some(local), Some(rect)) = (resp.pointer_local, resp.rect)
        {
            self.world_pos = to_world(local, scene);
            self.source = pending_source;
            self.menu.open_at(rect.min + local);
        }

        let chosen = self.menu.show(
            ui,
            "new_node_popup",
            Some(ctx.theme.new_node_popup_max_height),
            |ui, popup| palette_body(ui, popup, ctx),
        );

        if let Some(ChosenNode {
            node,
            def,
            bindings,
        }) = chosen
        {
            let view_node = ViewNode {
                id: node.id,
                pos: self.world_pos,
            };
            out.push(Intent::AddNode {
                view_node,
                node,
                def,
                bindings,
            });
            // If a dropped connection opened this popup, hand its source
            // back so the wire resumes floating — the user then clicks the
            // exact port to land it, rather than it auto-attaching.
            self.resume_floating = self.source;
        }
    }

    /// Take the source of a wire whose drop spawned a node this frame — the
    /// canvas re-floats it on `ConnectionUI`. `None` on a plain palette open.
    pub(crate) fn take_resume_floating(&mut self) -> Option<PortRef> {
        self.resume_floating.take()
    }
}

/// Record the palette's category columns and return the chosen entry, if
/// any. One column per category (an `hstack`); within a column the funcs are
/// a `wrap_vstack`, so a category with many funcs wraps into sub-columns once
/// it would exceed the popup's height cap (set by the caller): a bounded
/// stack constrains its children's main axis, so the cap flows down through
/// the column stacks into each func wrap. Everything hugs, so the popup is
/// only as wide/tall as its columns need. Funcs first, then this category's
/// shared (`Linked`) subgraph defs. The pick is owned, holding no `library`
/// borrow past the body.
fn palette_body(ui: &mut Ui, popup: &PopupHandle, ctx: &AppContext<'_>) -> Option<ChosenNode> {
    let mut chosen: Option<ChosenNode> = None;
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
                                for func in
                                    ctx.library.funcs.iter().filter(|f| f.category == category)
                                {
                                    if MenuItem::new(func.name.clone()).show(ui, popup).clicked() {
                                        let node: Node = func.into();
                                        let bindings = default_bindings(node.id, &func.inputs);
                                        chosen = Some(ChosenNode {
                                            node,
                                            def: None,
                                            bindings,
                                        });
                                    }
                                }
                                // Built-in special nodes of this category (their
                                // interface is hardcoded, not library-registered).
                                for &special in SPECIAL_NODES
                                    .iter()
                                    .filter(|s| s.func().category == category)
                                {
                                    if let Some(picked) = special_entry(ui, popup, special) {
                                        chosen = Some(picked);
                                    }
                                }
                                // Shared (`Linked`) subgraph defs of this
                                // category, after the funcs.
                                for def in ctx
                                    .library
                                    .subgraphs
                                    .iter()
                                    .filter(|d| d.category == category)
                                {
                                    if MenuItem::new(def.name.clone()).show(ui, popup).clicked() {
                                        // Localize on instance: drop an editable
                                        // `Local` copy that records its library `origin`.
                                        let mut local = def.fresh_copy();
                                        local.origin = Some(def.id);
                                        let node = Node::subgraph_instance(
                                            &local,
                                            SubgraphRef::Local(local.id),
                                        );
                                        let bindings = default_bindings(node.id, &local.inputs);
                                        chosen = Some(ChosenNode {
                                            node,
                                            def: Some(Box::new(local)),
                                            bindings,
                                        });
                                    }
                                }
                            });
                    });
            }
        });
    chosen
}

/// Every category that has a func *or* a shared subgraph def, sorted +
/// deduped — one popup column each.
fn sorted_categories<'a>(ctx: &'a AppContext<'_>) -> Vec<&'a str> {
    let mut cats: Vec<&str> = ctx
        .library
        .funcs
        .iter()
        .map(|f| f.category.as_str())
        .chain(ctx.library.subgraphs.iter().map(|d| d.category.as_str()))
        .chain(SPECIAL_NODES.iter().map(|s| s.func().category.as_str()))
        .collect();
    cats.sort();
    cats.dedup();
    cats
}

/// One palette entry for a built-in special node. Its `Func` (interface) is
/// hardcoded, not library-registered, so the spawned `Node` is a
/// `NodeKind::Special` named after that func, with its declared input defaults
/// seeded like a func node.
fn special_entry(ui: &mut Ui, popup: &PopupHandle, special: SpecialNode) -> Option<ChosenNode> {
    let func = special.func();
    if !MenuItem::new(func.name.clone()).show(ui, popup).clicked() {
        return None;
    }
    let mut node = Node::new(NodeKind::Special(special));
    node.name = func.name.clone();
    let bindings = default_bindings(node.id, &func.inputs);
    Some(ChosenNode {
        node,
        def: None,
        bindings,
    })
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
        let mut input = FuncInput::required("p", DataType::Float);
        input.default_value = default;
        input
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
