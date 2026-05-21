use glam::Vec2;
use palantir::{
    ClickOutside, Configure, MenuItem, Panel, Popup, Scroll, Sizing, Spacing, Text, Ui,
};
use scenarium::function::Func;
use scenarium::graph::Node;

use crate::app::AppContext;
use crate::gui::graph_ui::{outer_canvas_widget_id, to_world};
use crate::intent::Intent;
use crate::model::ViewNode;
use crate::scene::Scene;

/// Outer cap on the popup's height. Inside the popup body, a
/// `Scroll::vertical` takes the remaining space — when the category
/// columns add up to more than this, the user scrolls instead of the
/// popup spilling off-surface.
const NEW_NODE_POPUP_MAX_H: f32 = 400.0;

/// Right-click-on-canvas → popup that lists every `Func` in
/// `AppContext::func_lib` grouped by category. Clicking an entry emits
/// an `Intent::AddNode` placed at the click's world position (inner-
/// canvas pre-transform). Outside-click and Esc dismiss.
#[derive(Default, Debug)]
pub struct NewNodeUi {
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
    pub fn apply(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        out: &mut Vec<Intent>,
    ) {
        let resp = ui.response_for(outer_canvas_widget_id());
        if resp.secondary_clicked
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

        let mut chosen: Option<&Func> = None;
        let chrome = ui.theme.context_menu.panel.clone();
        // Cap the popup's height so it stays usable on small surfaces;
        // the inner `Scroll::vertical` handles overflow when the
        // function list exceeds the cap. Width hugs the content so the
        // popup is as wide as the widest column row, but no wider.
        let popup_resp = Popup::anchored_to(open.anchor)
            .click_outside(ClickOutside::Dismiss)
            .background(chrome)
            .id_salt("new_node_popup")
            .size((Sizing::Hug, Sizing::Hug))
            .max_size((f32::INFINITY, NEW_NODE_POPUP_MAX_H))
            .padding(Spacing::all(6.0))
            .show(ui, |ui, popup| {
                Scroll::vertical()
                    .id_salt("new_node_scroll")
                    .size((Sizing::Hug, Sizing::Fill(1.0)))
                    // `always_reserve` keeps the bar gutter on the
                    // pan axis whether or not content currently
                    // overflows. The popup hugs the scroll's outer
                    // size — without this, the gutter appears only
                    // once `Scroll.seen == true` (frame N pass B),
                    // the body grows by a bar width, and the popup
                    // re-places on frame N+1 → a visible shift of
                    // `bar_width` between the opening frame and the
                    // next input. Reserving unconditionally keeps the
                    // hugged width constant across passes.
                    .always_reserve()
                    .show(ui, |ui| {
                        Panel::hstack()
                            .id_salt("new_node_columns")
                            .size((Sizing::Hug, Sizing::Hug))
                            .gap(12.0)
                            .show(ui, |ui| {
                                for category in sorted_categories(ctx) {
                                    Panel::vstack()
                                        .id_salt(("new_node_col", category))
                                        .size((Sizing::Hug, Sizing::Hug))
                                        .gap(2.0)
                                        .show(ui, |ui| {
                                            Text::new(category.to_owned())
                                                .id_salt(("new_node_cat", category))
                                                .show(ui);
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
                                                    chosen = Some(func);
                                                }
                                            }
                                        });
                                }
                            });
                    });
            });

        if let Some(func) = chosen {
            out.push(add_node_intent(func, open.world_pos));
            self.state = None;
        } else if popup_resp.dismissed || popup_resp.close_requested {
            self.state = None;
        }
    }
}

fn sorted_categories<'a>(ctx: &'a AppContext<'_>) -> Vec<&'a str> {
    let mut cats: Vec<&str> = ctx
        .func_lib
        .funcs
        .iter()
        .map(|f| f.category.as_str())
        .collect();
    cats.sort();
    cats.dedup();
    cats
}

fn add_node_intent(func: &Func, world_pos: Vec2) -> Intent {
    let node: Node = func.into();
    let view_node = ViewNode {
        id: node.id,
        pos: world_pos,
    };
    Intent::AddNode { view_node, node }
}
