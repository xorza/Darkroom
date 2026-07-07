use glam::Vec2;
use palantir::{
    Configure, MenuItem, Panel, PopupHandle, Scroll, Sizing, Spacing, Text, TextEdit, Tooltip, Ui,
    WidgetId,
};
use scenarium::graph::NodeId;
use scenarium::graph::subgraph::{SubgraphDef, SubgraphRef};
use scenarium::graph::{Binding, InputPort, Node, NodeKind};
use scenarium::node::function::FuncInput;
use scenarium::node::special::{ALL as SPECIAL_NODES, SpecialNode};

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
    /// Live text of the palette's search field. Cleared on each open;
    /// case-insensitively filters the listed entries by name (a matching
    /// category name shows that whole column). Empty ⇒ everything shows.
    query: String,
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
        let mut just_opened = false;
        if (pending_source.is_some() || gesture == Some(CanvasGesture::NewNode))
            && let (Some(local), Some(rect)) = (resp.pointer_local, resp.rect)
        {
            self.world_pos = to_world(local, scene);
            self.source = pending_source;
            self.menu.open_at(rect.min + local);
            // Fresh open: empty the filter and focus the search field this
            // frame so the user can type straight away.
            self.query.clear();
            just_opened = true;
        }

        // Cap the palette height to the window so a short window scrolls
        // the overflow (via the inner vertical `Scroll`) instead of
        // running off-screen. The popup's `max_size` height bounds the
        // whole popup; the search row sits above a `Scroll` whose own cap
        // (`max_height` minus the search row) keeps it from eating the
        // header's space — a `Hug` scroll otherwise claims the full cap.
        let surface = ui.display().logical_rect();
        let max_height = ctx
            .theme
            .new_node_popup_max_height
            .min(surface.size.h - 16.0)
            .max(120.0);
        let scroll_cap = (max_height - SEARCH_ROW_ALLOWANCE).max(80.0);
        let query = &mut self.query;
        let chosen = self
            .menu
            .show(ui, "new_node_popup", Some(max_height), |ui, popup| {
                palette_body(ui, popup, ctx, query, scroll_cap, just_opened)
            });

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

/// Gap (px) below the search field, before the results scroll.
const SEARCH_ROW_GAP: f32 = 8.0;

/// Vertical space (px) the search row (field + its [`SEARCH_ROW_GAP`]) and
/// popup padding claim above the scrolling results, subtracted from the
/// popup's height cap to size the inner `Scroll`.
const SEARCH_ROW_ALLOWANCE: f32 = 48.0;

/// Record the palette: a search field pinned at the top, then the category
/// columns (one `hstack` column per category) inside a vertical `Scroll`.
/// Entries whose name (case-insensitively) contains `query` show; a category
/// whose *own* name matches shows its whole column. Empty `query` ⇒ all show.
///
/// The `Scroll` pans the overflow when the window is too short. It carries an
/// explicit `max_size` (`scroll_cap`) rather than leaning on the popup's cap:
/// a `Hug` scroll in a capped popup otherwise claims the full cap and spills
/// over the search row. `focus` grabs the field on the opening frame so the
/// user types immediately. The pick is owned, holding no `library` borrow
/// past the body.
fn palette_body(
    ui: &mut Ui,
    popup: &PopupHandle,
    ctx: &AppContext<'_>,
    query: &mut String,
    scroll_cap: f32,
    focus: bool,
) -> Option<ChosenNode> {
    let mut chosen: Option<ChosenNode> = None;

    let search_id = WidgetId::from_hash("new_node_search");
    TextEdit::new(query)
        .id(search_id)
        .placeholder("Search…")
        .style(ctx.theme.inline_rename.text_edit.clone())
        .size((Sizing::Fill(1.0), Sizing::Hug))
        .min_size((200.0, 0.0))
        .margin(Spacing::new(0.0, 0.0, 0.0, SEARCH_ROW_GAP))
        .show(ui);
    if focus {
        ui.request_focus(Some(search_id));
    }
    let query_lc = query.to_lowercase();

    Scroll::vertical()
        .id_salt("new_node_scroll")
        .size((Sizing::Hug, Sizing::Hug))
        .max_size((f32::INFINITY, scroll_cap))
        .show(ui, |ui| {
            Panel::hstack()
                .id_salt("new_node_columns")
                .size((Sizing::Hug, Sizing::Hug))
                .gap(12.0)
                .show(ui, |ui| {
                    for category in sorted_categories(ctx) {
                        // A matching category name reveals its whole column;
                        // otherwise each entry is filtered by its own name.
                        let cat_match = name_matches(category, &query_lc);
                        let shows = |name: &str| cat_match || name_matches(name, &query_lc);
                        let has_any = ctx
                            .library
                            .funcs
                            .iter()
                            .any(|f| f.category == category && shows(&f.name))
                            || SPECIAL_NODES
                                .iter()
                                .any(|s| s.func().category == category && shows(&s.func().name))
                            || ctx
                                .library
                                .subgraphs
                                .iter()
                                .any(|d| d.category == category && shows(&d.name));
                        if !has_any {
                            continue;
                        }
                        Panel::vstack()
                            .id_salt(("new_node_col", category))
                            .size((Sizing::Hug, Sizing::Hug))
                            .gap(4.0)
                            .show(ui, |ui| {
                                Text::new(category.to_owned())
                                    .id_salt(("new_node_cat", category))
                                    .show(ui);
                                Panel::vstack()
                                    .id_salt(("new_node_funcs", category))
                                    .size((Sizing::Hug, Sizing::Hug))
                                    .gap(2.0)
                                    .show(ui, |ui| {
                                        for func in
                                            ctx.library.funcs.iter().filter(|f| {
                                                f.category == category && shows(&f.name)
                                            })
                                        {
                                            let resp =
                                                MenuItem::new(func.name.clone()).show(ui, popup);
                                            let clicked = resp.clicked();
                                            if let Some(desc) = &func.description {
                                                Tooltip::for_(&resp.snapshot())
                                                    .text(desc.clone())
                                                    .show(ui);
                                            }
                                            if clicked {
                                                let node: Node = func.into();
                                                let bindings =
                                                    default_bindings(node.id, &func.inputs);
                                                chosen = Some(ChosenNode {
                                                    node,
                                                    def: None,
                                                    bindings,
                                                });
                                            }
                                        }
                                        // Built-in special nodes of this category (their
                                        // interface is hardcoded, not library-registered).
                                        for &special in SPECIAL_NODES.iter().filter(|s| {
                                            s.func().category == category && shows(&s.func().name)
                                        }) {
                                            if let Some(picked) = special_entry(ui, popup, special)
                                            {
                                                chosen = Some(picked);
                                            }
                                        }
                                        // Shared (`Linked`) subgraph defs of this
                                        // category, after the funcs.
                                        for def in
                                            ctx.library.subgraphs.iter().filter(|d| {
                                                d.category == category && shows(&d.name)
                                            })
                                        {
                                            if MenuItem::new(def.name.clone())
                                                .show(ui, popup)
                                                .clicked()
                                            {
                                                // Localize on instance: drop an editable
                                                // `Local` copy that records its library `origin`.
                                                let mut local = def.fresh_copy();
                                                local.origin = Some(def.id);
                                                let node = Node::subgraph_instance(
                                                    &local,
                                                    SubgraphRef::Local(local.id),
                                                );
                                                let bindings =
                                                    default_bindings(node.id, &local.inputs);
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
        });
    chosen
}

/// Case-insensitive substring match used by the palette search. An empty
/// (already-lowercased) query matches everything.
fn name_matches(name: &str, query_lc: &str) -> bool {
    query_lc.is_empty() || name.to_lowercase().contains(query_lc)
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
    let resp = MenuItem::new(func.name.clone()).show(ui, popup);
    let clicked = resp.clicked();
    if let Some(desc) = &func.description {
        Tooltip::for_(&resp.snapshot()).text(desc.clone()).show(ui);
    }
    if !clicked {
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

    #[test]
    fn name_matches_is_case_insensitive_substring_with_empty_query_wildcard() {
        // Empty query is the "show everything" wildcard.
        assert!(name_matches("Gaussian Blur", ""));
        assert!(name_matches("", ""));
        // Case-insensitive substring anywhere in the name. Caller passes an
        // already-lowercased query, so only the name is folded here.
        assert!(name_matches("Gaussian Blur", "blur"));
        assert!(name_matches("Gaussian Blur", "gauss"));
        assert!(name_matches("Gaussian Blur", "an bl"));
        // Non-substring and wrong-fragment queries reject.
        assert!(!name_matches("Gaussian Blur", "sharpen"));
        assert!(!name_matches("Blur", "blurry"));
        // A non-lowercased query never matches a lowercased name — the
        // contract is "query already lowercased", so this documents that a
        // caller who forgets to fold gets no false positives.
        assert!(!name_matches("blur", "BLUR"));
    }
}
