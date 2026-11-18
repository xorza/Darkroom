//! What darkroom answers about each dock tab: its label and the view it
//! draws.
//!
//! The dock itself is palantir's ([`palantir::DockView`]); this is the
//! whole of darkroom's side of it. Adding a pane *kind* is a new arm in
//! [`tab_label`] and one in [`DockPanes::body`], and nothing else.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use glam::Vec2;
use palantir::{
    Configure, DockDrop, DockOperation, DockTabMenu, DockTabs, EditableLabel, InternedStr,
    MenuItem, Size, SplitSide, SplitSize, TabChip, Ui, WidgetId, fmt,
};
use scenarium::{CompoundId, GraphId, Library};

use crate::core::document;
use crate::core::document::Document;
use crate::core::document::inspect_target::InspectTarget;
use crate::core::document::pane_kind::PaneKind;
use crate::core::document::tab_ref::TabRef;
use crate::core::document::view_target::ViewTarget;
use crate::core::edit::compound_intent::CompoundIntent;
use crate::core::io::preferences::Preferences;
use crate::gui::frame::doc_ctx::DocCtx;
use crate::gui::frame::stage::Stage;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::GraphUi;
use crate::gui::pane::image_viewer::{self, ImageViewer, PreviewDisplay, ViewerPane, ViewerWrites};
use crate::gui::pane::inspector;
use crate::gui::pane::preferences::PreferencesForm;
use crate::gui::pane::reference;
use crate::gui::widgets::format;

/// Everything a pane's body needs, borrowed for one record pass.
///
/// A bundle rather than closures, which would each capture the same borrows.
#[derive(Debug)]
pub(crate) struct DockPanes<'a> {
    pub(crate) stage: Stage<'a, DocCtx<'a>>,
    /// One canvas per graph a tab shows; see `MainWindow::graph_panes`.
    pub(crate) graph_panes: &'a mut BTreeMap<GraphId, GraphUi>,
    pub(crate) image_viewers: &'a mut HashMap<ViewTarget, ImageViewer>,
    pub(crate) preferences: &'a mut Preferences,
}

/// The name a tab's chip shows, written on demand: a compound's pane names the compound, and an
/// inspector held on a node names it, the node's graph resolved against `library`.
pub(crate) fn tab_label<'a>(
    document: &'a Document,
    library: &'a Library,
    tab: TabRef,
) -> TabLabel<'a> {
    match tab {
        TabRef::Graph(GraphId::Main) => TabLabel::Plain("Main"),
        TabRef::Graph(GraphId::Compound(id)) => TabLabel::Plain(
            &document
                .graphs
                .compound(id)
                .expect("a compound's pane closes with it")
                .name,
        ),
        TabRef::Preferences => TabLabel::Plain(PaneKind::Preferences.label()),
        TabRef::Reference => TabLabel::Plain(PaneKind::Reference.label()),
        TabRef::Viewer(ViewTarget::Active) => TabLabel::Plain(PaneKind::Viewer.label()),
        TabRef::Viewer(ViewTarget::Node(program_id)) => TabLabel::Held {
            pane: PaneKind::Viewer,
            name: image_viewer::node_label(document, library, program_id),
        },
        TabRef::Inspector(InspectTarget::Selection) => TabLabel::Plain(PaneKind::Inspector.label()),
        TabRef::Inspector(InspectTarget::Node { addr, .. }) => {
            let graphs = &document.graphs;
            let node = graphs
                .graph(addr.graph)
                .find(addr.node_id)
                .expect("a held inspector closes with its node");
            TabLabel::Held {
                pane: PaneKind::Inspector,
                name: format::node_name(document::title(node, graphs.scope(library, addr.graph))),
            }
        }
    }
}

/// A tab chip's name, as [`tab_label`] gives it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum TabLabel<'a> {
    Plain(&'a str),
    /// A pane held on the node of this name.
    Held {
        pane: PaneKind,
        name: &'a str,
    },
}

impl Display for TabLabel<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plain(label) => f.write_str(label),
            Self::Held { pane, name } => write!(f, "{} \u{b7} {name}", pane.label()),
        }
    }
}

impl DockTabs for DockPanes<'_> {
    type Tab = TabRef;

    fn label(&mut self, ui: &mut Ui, tab: TabRef) -> InternedStr {
        let ctx = self.stage.ctx;
        fmt!(ui, "{}", tab_label(ctx.document(), ctx.library(), tab))
    }

    fn body(&mut self, ui: &mut Ui, tab: TabRef, size: Option<Size>) {
        let ctx = self.stage.ctx;
        let display = ui.display();
        match tab {
            TabRef::Graph(graph) => {
                let graph_ctx = GraphCtx::of(ctx, graph, display);
                self.graph_panes
                    .entry(graph)
                    .or_default()
                    .record(ui, &mut self.stage.with(graph_ctx));
            }
            TabRef::Preferences => {
                PreferencesForm {
                    theme: ctx.theme(),
                    out: self.stage.out,
                }
                .show(ui, self.preferences);
            }
            TabRef::Reference => reference::show(ui, ctx.theme()),
            TabRef::Inspector(target) => {
                // A held inspector reads its node's graph through the instance it was pinned to;
                // the selection's is the active graph's, through the instance its pane shows.
                let graph_ctx = match &target {
                    InspectTarget::Node { addr, through } => {
                        GraphCtx::of(ctx, addr.graph, display).through_instance(through.as_ref())
                    }
                    InspectTarget::Selection => GraphCtx::of(ctx, ctx.active(), display),
                };
                inspector::show(ui, &mut self.stage.with(graph_ctx), target);
            }
            TabRef::Viewer(target) => {
                let program_id = match target {
                    ViewTarget::Node(program_id) => Some(program_id),
                    ViewTarget::Active => ctx.document().active_preview,
                };
                let Some(program_id) = program_id else {
                    image_viewer::nothing_to_show(ui, ctx.theme());
                    return;
                };
                let title = image_viewer::node_label(ctx.document(), ctx.library(), program_id);
                let previews = ctx.run_state().previews();
                let viewer = self
                    .image_viewers
                    .entry(target)
                    .or_insert_with(|| ImageViewer::new(program_id));
                // The following viewer starts over, at fit, on the next preview.
                if viewer.program_id() != program_id {
                    *viewer = ImageViewer::new(program_id);
                }
                // Viewer-toolbar edits go through the same in-place preferences path as the
                // Preferences tab.
                let pane = ViewerPane {
                    theme: ctx.theme(),
                    title,
                    previews,
                    status: ctx.run_state().exec_status(program_id),
                    size: size.map(Vec2::from),
                    pinnable: target == ViewTarget::Active,
                    display: PreviewDisplay::of(ctx.document(), program_id),
                };
                viewer.show(
                    ui,
                    pane,
                    ViewerWrites {
                        preferences: &mut self.preferences.viewer,
                        out: self.stage.out,
                    },
                );
            }
        }
    }

    /// A compound's tab renames it in place: a double click on the chip, or Rename in its menu,
    /// opens an editor of the name, as Excel's sheet tab does
    /// ([rename a worksheet](https://support.microsoft.com/en-us/office/rename-a-worksheet-3f1f7148-ee83-404d-8ef0-9ff99fbad1f9)).
    /// Enter or a blur commits, Escape keeps the name; the chip keeps its click, drag and close
    /// button around the editor.
    fn chip(&mut self, ui: &mut Ui, tab: TabRef, chip: &TabChip) {
        let TabRef::Graph(GraphId::Compound(compound)) = tab else {
            chip.show_label(ui);
            return;
        };
        let editor = rename_wid(compound);
        if ui.response_for(chip.id).left.double_clicked() {
            EditableLabel::start_editing(ui, editor);
        }
        if !EditableLabel::is_editing(ui, editor) {
            chip.show_label(ui);
            return;
        }
        let name = &self
            .stage
            .ctx
            .document()
            .graphs
            .compound(compound)
            .expect("a compound's pane closes with it")
            .name;
        let label = EditableLabel::new(name.as_str())
            .id(editor)
            .style(&self.stage.ctx.theme().flat_text_edit)
            .max_chars(format::NAME_MAX_CHARS)
            .show(ui);
        if let Some(to) = label.committed_text {
            self.stage
                .out
                .push_compound(CompoundIntent::Rename { id: compound, to });
        }
    }

    /// Every tab has a menu: the splits for any, and Rename for a compound's.
    fn has_tab_menu(&mut self, _tab: TabRef) -> bool {
        true
    }

    /// Every tab except the pinned main graph carries a close button.
    fn closable(&mut self, tab: TabRef) -> bool {
        tab != TabRef::Graph(GraphId::Main)
    }

    /// The keyboard-free, discoverable route to the same split a chip
    /// drag performs.
    fn tab_menu(&mut self, ui: &mut Ui, menu: DockTabMenu<'_, TabRef>) {
        if let TabRef::Graph(GraphId::Compound(compound)) = menu.tab
            && MenuItem::new("Rename").show(ui).left.clicked()
        {
            EditableLabel::start_editing(ui, rename_wid(compound));
        }
        let right = MenuItem::new("Split right").show(ui).left.clicked();
        let down = MenuItem::new("Split down").show(ui).left.clicked();
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
                    size: SplitSize::HALF,
                },
            });
        }
    }
}

/// The editor of `compound`'s name in its tab's chip: keyed by the compound, as it has one tab.
pub(crate) fn rename_wid(compound: CompoundId) -> WidgetId {
    WidgetId::from_hash(("darkroom.dock.rename", compound))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use scenarium::{Compound, CompoundId, GraphId, InstancePath, Interface, Node, NodeAddr};

    use crate::core::document::inspect_target::InspectTarget;
    use crate::core::document::internals::{DocumentFixture, insert_compound};
    use crate::core::document::tab_ref::TabRef;
    use crate::core::document::view_target::ViewTarget;
    use crate::core::preview_sink;
    use crate::gui::main_window::dock_panes::tab_label;

    /// Each tab names what it shows: the main graph's pane "Graph", a compound's pane the
    /// compound, and a viewer and a pinned inspector the node inside the compound they show.
    #[test]
    fn a_tab_names_what_it_shows_in_any_graph() {
        let mut fixture = DocumentFixture::default();
        let id = CompoundId::from_u128(1);
        let mut compound = Compound::new("Denoise", Interface::default());
        let mut preview = Node::from(&preview_sink::func(Arc::default()));
        preview.name = "Shown".to_string();
        let shown = compound.graph.add(preview);
        insert_compound(&mut fixture.document, id, compound);
        let instance = fixture.add_instance(id);
        let DocumentFixture { document, library } = fixture;
        let label = |tab| tab_label(&document, &library, tab).to_string();

        assert_eq!(label(TabRef::Graph(GraphId::Main)), "Main");
        assert_eq!(label(TabRef::Graph(GraphId::Compound(id))), "Denoise");
        let path = InstancePath::new(&[instance]);
        assert_eq!(
            label(TabRef::Viewer(ViewTarget::Node(path.node(shown)))),
            "Viewer \u{b7} Shown"
        );
        assert_eq!(label(TabRef::Viewer(ViewTarget::Active)), "Viewer");
        assert_eq!(
            label(TabRef::Inspector(InspectTarget::Node {
                addr: NodeAddr::new(GraphId::Compound(id), shown),
                through: Some(path),
            })),
            "Inspector \u{b7} Shown"
        );
    }
}
