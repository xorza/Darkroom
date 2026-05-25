use glam::Vec2;
use palantir::{
    ClickOutside, Configure, MenuItem, Panel, Popup, Scroll, Sizing, Spacing, Text, Ui,
};
use scenarium::function::Func;
use scenarium::graph::Node;

use crate::app::AppContext;
use crate::document::view_node::ViewNode;
use crate::edit::intent::Intent;
use crate::gui::canvas::{outer_canvas_widget_id, to_world};
use crate::scene::Scene;

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
            .max_size((f32::INFINITY, ctx.theme.new_node_popup_max_height))
            .padding(Spacing::all(6.0))
            .show(ui, |ui, popup| {
                Scroll::vertical()
                    .id_salt("new_node_scroll")
                    .size((Sizing::Hug, Sizing::Fill(1.0)))
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
