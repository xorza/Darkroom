//! What darkroom answers about each dock tab: its label, its badge, and
//! the view it draws.
//!
//! The dock itself is palantir's ([`palantir::DockView`]); this is the
//! whole of darkroom's side of it. Adding a pane *kind* is a new arm in
//! [`DockPanes::content`] and one in [`DockPanes::title`], and nothing
//! else.

use std::collections::HashMap;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use glam::Vec2;
use palantir::{
    DockDrop, DockOperation, DockTabMenu, DockTabs, InternedStr, MenuItem, Size, SplitSide,
    TabBadge, Ui, fmt,
};
use scenarium::NodeId;

use crate::core::document::Document;
use crate::core::document::inspect_target::InspectTarget;
use crate::core::document::tab_ref::TabRef;
use crate::core::io::preferences::Preferences;
use crate::gui::frame::requests::Requests;
use crate::gui::frame::window_ctx::WindowCtx;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::GraphUi;
use crate::gui::pane::image_viewer::{self, ImageViewer, ViewerPane, ViewerWrites};
use crate::gui::pane::inspector;
use crate::gui::pane::preferences::PreferencesForm;

/// Everything a pane's body needs, borrowed for one record pass.
///
/// A bundle rather than a closure, because the dock asks six questions
/// per tab and a pile of builder closures would carry the same six
/// borrows one capture at a time.
#[derive(Debug)]
pub(crate) struct DockPanes<'a> {
    pub(crate) ctx: WindowCtx<'a>,
    pub(crate) graph_ui: &'a mut GraphUi,
    pub(crate) image_viewers: &'a mut HashMap<NodeId, ImageViewer>,
    pub(crate) preferences: &'a mut Preferences,
    pub(crate) out: &'a mut Requests,
}

/// The name a tab's chip shows, written on demand: an inspector held on a node names it.
pub(crate) fn tab_label(document: &Document, tab: TabRef) -> TabLabel<'_> {
    match tab {
        TabRef::Graph => TabLabel::Plain("Graph"),
        TabRef::Preferences => TabLabel::Plain("Preferences"),
        TabRef::ImageViewer(node_id) => {
            TabLabel::Plain(image_viewer::node_label(document, node_id))
        }
        TabRef::Inspector(InspectTarget::Selection) => TabLabel::Plain("Inspector"),
        TabRef::Inspector(InspectTarget::Node(node_id)) => TabLabel::Inspector(
            document
                .graph
                .find(node_id)
                .map_or("", |node| node.name.as_str()),
        ),
    }
}

/// A tab chip's name, as [`tab_label`] gives it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum TabLabel<'a> {
    Plain(&'a str),
    /// An inspector held on the node of this name.
    Inspector(&'a str),
}

impl Display for TabLabel<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plain(label) => f.write_str(label),
            Self::Inspector(name) => write!(f, "Inspector \u{b7} {name}"),
        }
    }
}

impl DockTabs for DockPanes<'_> {
    type Tab = TabRef;

    fn title(&mut self, ui: &mut Ui, tab: TabRef) -> InternedStr {
        fmt!(ui, "{}", tab_label(self.ctx.document(), tab))
    }

    fn content(&mut self, ui: &mut Ui, tab: TabRef, size: Option<Size>) {
        let ctx = self.ctx;
        let app = ctx.app();
        match tab {
            TabRef::Graph => {
                // The context carries everything the canvas reads — theme
                // and run included, through the `ctx` it is composed from.
                self.graph_ui.draw(ui, GraphCtx::new(ctx), self.out);
            }
            TabRef::Preferences => {
                PreferencesForm {
                    theme: app.theme(),
                    out: self.out,
                }
                .show(ui, self.preferences);
            }
            TabRef::Inspector(target) => {
                inspector::show(ui, GraphCtx::new(ctx), target, self.out);
            }
            TabRef::ImageViewer(node_id) => {
                let title = image_viewer::node_label(ctx.document(), node_id);
                let previews = app.run_state().previews();
                let viewer = self
                    .image_viewers
                    .entry(node_id)
                    .or_insert_with(|| ImageViewer::new(node_id));
                // Viewer-toolbar edits ride the same in-place preferences path
                // as the Preferences tab, and report on the same queue.
                let pane = ViewerPane {
                    theme: app.theme(),
                    title,
                    previews,
                    status: app.run_state().exec_status(node_id),
                    size: size.map(Vec2::from),
                };
                viewer.show(
                    ui,
                    pane,
                    ViewerWrites {
                        preferences: &mut self.preferences.viewer,
                        out: self.out,
                    },
                );
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
        if self.ctx.open().dirty {
            TabBadge::On
        } else {
            TabBadge::Idle
        }
    }

    /// The keyboard-free, discoverable route to the same split a chip
    /// drag performs.
    fn tab_menu(&mut self, ui: &mut Ui, menu: DockTabMenu<'_, TabRef>) {
        let right = MenuItem::new("Split right")
            .show(ui, menu.close)
            .left
            .clicked();
        let down = MenuItem::new("Split down")
            .show(ui, menu.close)
            .left
            .clicked();
        let side = if down {
            Some(SplitSide::Bottom)
        } else {
            right.then_some(SplitSide::Right)
        };
        if let Some(side) = side {
            menu.operations.push(DockOperation::MoveTab {
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
    use palantir::{DockView, TabStrip};

    use crate::core::document::internals::DocumentFixture;
    use crate::core::document::tab_ref::TabRef;
    use crate::gui::app::session::internals::SessionHarness;

    /// Which tab kinds reserve the unsaved-changes dot is darkroom's own
    /// sentence, and the dot is a visibility change rather than a layout
    /// one: saving — or making the first edit after a save — must leave
    /// the graph chip exactly the size it was, or every chip to its right
    /// would shift.
    #[test]
    fn the_badge_box_is_reserved_by_the_graph_tab_alone_and_never_resizes_it() {
        // Preferences beside the graph, so the test also covers a chip
        // that reserves nothing: it must not grow a slot of its own.
        let mut h = SessionHarness::new(DocumentFixture::sample().with_tab(TabRef::Preferences));
        let strip = {
            let layout = &h.session.open.document.layout;
            DockView::strip_id(layout, layout.primary().id())
        };
        let chip = |tab: TabRef| TabStrip::chip_id(strip, DockView::<TabRef>::tab_key(tab));
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
