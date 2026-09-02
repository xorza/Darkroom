//! What darkroom answers about each dock tab: its label, its badge, and
//! the view it draws.
//!
//! The dock itself is palantir's ([`palantir::DockView`]); this is the
//! whole of darkroom's side of it. Adding a pane *kind* is a new arm in
//! [`DockPanes::content`] and one in [`DockPanes::title`], and nothing
//! else.

use std::collections::HashMap;

use glam::Vec2;
use palantir::{
    DockDrop, DockOp, DockTabMenu, DockTabs, InternedStr, MenuItem, SplitSide, TabBadge, Ui,
};
use scenarium::{NodeId, OutputTypes};

use crate::core::document::TabRef;
use crate::core::io::preferences::Preferences;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::prefs::PrefsCommand;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph::GraphUI;
use crate::gui::pane::preferences;
use crate::gui::pane::viewer::{self, ImageViewer};
use crate::gui::requests::Requests;
use crate::gui::window::ctx::WindowCtx;

/// Everything a pane's body needs, borrowed for one record pass.
///
/// A bundle rather than a closure, because the dock asks six questions
/// per tab and a pile of builder closures would carry the same six
/// borrows one capture at a time.
#[derive(Debug)]
pub(crate) struct DockPanes<'a> {
    pub(crate) cx: WindowCtx<'a>,
    pub(crate) graph_ui: &'a mut GraphUI,
    pub(crate) image_viewers: &'a mut HashMap<NodeId, ImageViewer>,
    pub(crate) output_types: &'a mut OutputTypes,
    pub(crate) prefs: &'a mut Preferences,
    pub(crate) out: &'a mut Requests,
}

impl DockTabs for DockPanes<'_> {
    type Tab = TabRef;

    fn title(&mut self, ui: &mut Ui, tab: TabRef) -> InternedStr {
        let doc = self.cx.document();
        ui.intern(match tab {
            TabRef::Graph => "main",
            TabRef::Preferences => "preferences",
            TabRef::ImageViewer(node_id) => viewer::node_label(doc, node_id),
        })
    }

    fn content(&mut self, ui: &mut Ui, tab: TabRef, size: Option<Vec2>) {
        let cx = self.cx;
        let app = cx.app();
        match tab {
            TabRef::Graph => {
                // The context carries everything the canvas reads — theme
                // and run included, through the `cx` it is composed from.
                self.graph_ui
                    .draw(ui, GraphCtx::new(cx, self.output_types), self.out);
            }
            TabRef::Preferences => {
                preferences::show(ui, app.theme(), self.prefs, self.out);
            }
            TabRef::ImageViewer(node_id) => {
                let title = viewer::node_label(cx.document(), node_id);
                let previews = &app.run_state().previews;
                let viewer = self
                    .image_viewers
                    .entry(node_id)
                    .or_insert_with(|| ImageViewer::new(node_id));
                // Viewer-toolbar edits ride the same in-place prefs path
                // as the Preferences tab.
                if viewer.show(
                    ui,
                    app.theme(),
                    &mut self.prefs.viewer,
                    title,
                    previews,
                    size,
                ) {
                    self.out.push_app(AppCommand::Prefs(PrefsCommand::Changed));
                }
            }
        }
    }

    /// Every tab except the pinned graph carries a close button.
    fn closable(&mut self, tab: TabRef) -> bool {
        tab != TabRef::Graph
    }

    /// Which tabs show the open document's own content, and so carry its
    /// unsaved-changes dot. Preferences are application state and a
    /// viewer shows a run's output — neither is in the file, so neither
    /// reserves the slot.
    ///
    /// The graph tab reserves it on every frame, inked or not: a dot that
    /// came and went would resize the chip on every save and shuffle
    /// every chip to its right.
    fn badge(&mut self, tab: TabRef) -> TabBadge {
        if tab != TabRef::Graph {
            return TabBadge::None;
        }
        if self.cx.open().dirty {
            TabBadge::On
        } else {
            TabBadge::Idle
        }
    }

    /// The keyboard-free, discoverable route to the same split a chip
    /// drag performs.
    fn tab_menu(&mut self, ui: &mut Ui, menu: DockTabMenu<'_, TabRef>) {
        let mut side = None;
        if MenuItem::new("Split right")
            .show(ui, menu.close)
            .left
            .clicked()
        {
            side = Some(SplitSide::Right);
        }
        if MenuItem::new("Split down")
            .show(ui, menu.close)
            .left
            .clicked()
        {
            side = Some(SplitSide::Bottom);
        }
        if let Some(side) = side {
            menu.ops.push(DockOp::MoveTab {
                tab: menu.tab,
                to: DockDrop::Split {
                    group: menu.group,
                    side,
                },
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use palantir::{DockState, TabStrip};

    use crate::core::document::TabRef;
    use crate::core::document::harness::DocFixture;
    use crate::gui::app::session::harness::SessionHarness;

    /// Which tab kinds reserve the unsaved-changes dot is darkroom's own
    /// sentence, and the dot is a visibility change rather than a layout
    /// one: saving — or making the first edit after a save — must leave
    /// the graph chip exactly the size it was, or every chip to its right
    /// would shift.
    #[test]
    fn the_badge_box_is_reserved_by_the_graph_tab_alone_and_never_resizes_it() {
        // Preferences beside the graph, so the test also covers a chip
        // that reserves nothing: it must not grow a slot of its own.
        let mut h = SessionHarness::new(DocFixture::sample().with_tab(TabRef::Preferences));
        let strip = {
            let layout = &h.session.open.document.layout;
            layout.strip_id(layout.primary().id)
        };
        let chip = |tab: TabRef| TabStrip::chip_id(strip, DockState::<TabRef>::tab_key(tab));
        let badge = |tab: TabRef| chip(tab).with("badge");

        h.session.open.dirty = false;
        h.prime(3);
        let saved_chip =
            h.ui.rect(chip(TabRef::Graph))
                .expect("the graph chip records");
        let saved_dot =
            h.ui.rect(badge(TabRef::Graph))
                .expect("a saved document still reserves the dot's box");

        h.session.open.dirty = true;
        h.prime(3);
        let unsaved_chip = h.ui.rect(chip(TabRef::Graph)).expect("still there");
        let unsaved_dot = h.ui.rect(badge(TabRef::Graph)).expect("still there");

        assert_eq!(
            (saved_chip.size.w, saved_chip.size.h),
            (unsaved_chip.size.w, unsaved_chip.size.h),
            "the dot resized the graph chip: {saved_chip:?} saved against {unsaved_chip:?} unsaved",
        );
        assert_eq!(
            (saved_dot.size.w, saved_dot.size.h),
            (unsaved_dot.size.w, unsaved_dot.size.h),
            "an inked dot fills exactly the box the saved document reserved",
        );
        assert!(
            unsaved_dot.max().x <= unsaved_chip.max().x,
            "the dot overflowed its chip: {unsaved_dot:?} in {unsaved_chip:?}",
        );
        assert!(
            h.ui.rect(badge(TabRef::Preferences)).is_none(),
            "preferences are not in the document and reserve no dot",
        );
    }
}
