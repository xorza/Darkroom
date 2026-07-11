use std::collections::HashMap;
use std::mem::take;

use aperture::{
    Align, Background, Configure, Panel, Sizing, SplitHalf, Splitter, Ui, VAlign, WidgetId,
};

use crate::core::document::dock::{
    DockLayout, DockNode, DockPath, DockSplit, NodeIdx, SplitDir, TabGroup, TabGroupId,
};
use crate::core::document::{Document, GraphRef, PortRef, TabRef};
use crate::core::edit::intent::{DockIntent, Intent};
use crate::core::io::preferences::Preferences;
use crate::gui::UiAction;
use crate::gui::app::AppContext;
use crate::gui::app::commands::AppCommand;
use crate::gui::canvas::GraphUI;
use crate::gui::graph_toolbar;
use crate::gui::image_viewer::{self, ImageViewer};
use crate::gui::menu_bar;
use crate::gui::node::emit_subgraph_opens;
use crate::gui::preferences_view;
use crate::gui::scene::Scene;
use crate::gui::status_bar;
use crate::gui::tab_bar::{self, TabLabel};

/// Smallest a dock pane can be squeezed on its split axis, in logical px.
const MIN_PANE: f32 = 220.0;

/// Top of darkroom's UI tree. Owns every persistent UI scope; adding a
/// new top-level pane *kind* is a new arm in `render_group`'s dispatch.
#[derive(Debug)]
pub(crate) struct MainWindow {
    pub(crate) graph_ui: GraphUI,
    /// One full-resolution image-viewer pane per open viewer tab
    /// ([`TabRef::ImageViewer`]), keyed by the port it shows. Fed by
    /// `Editor` (preview clicks + after-run refreshes), which also prunes
    /// entries whose tab closed — dropping one frees its texture.
    pub(crate) image_viewers: HashMap<PortRef, ImageViewer>,
    first_frame: bool,
}

/// Stable id for a group's pane container — the rect drop-zone math and
/// hover tests will key off (drag-docking phase).
fn pane_wid(group: TabGroupId) -> WidgetId {
    WidgetId::from_hash(("dock.pane", group))
}

/// Stable id for the splitter at a tree path.
fn splitter_wid(path: DockPath) -> WidgetId {
    WidgetId::from_hash(("dock.splitter", path))
}

/// Borrows threaded through the dock-tree walk — one bundle instead of a
/// six-way parameter fan-out at every recursion level (the
/// [`AppContext`] pattern, walk-scoped and carrying the mutable halves).
struct DockWalk<'a> {
    ctx: &'a AppContext<'a>,
    scene: &'a Scene,
    prefs: &'a mut Preferences,
    doc: &'a Document,
    out: &'a mut Vec<Intent>,
    command: &'a mut Option<AppCommand>,
}

impl MainWindow {
    /// Navigation scan: surface tab activate/close and subgraph-open
    /// requests from *last* frame's responses (`scene` is the
    /// last-rendered graph, which is what carried the clicked chips).
    /// `App` runs this at the top of the frame so a switch applies before
    /// the record — the switched-to graph records in Pass A and its
    /// connections draw in Pass B, no first-frame gap.
    pub(crate) fn scan_navigation(
        &self,
        ui: &Ui,
        doc: &Document,
        scene: &Scene,
        actions: &mut Vec<UiAction>,
    ) {
        tab_bar::emit_tab_actions(ui, &doc.layout, actions);
        emit_subgraph_opens(ui, scene, actions);
        self.graph_ui
            .inspectors
            .emit_preview_opens(ui, scene, actions);
    }

    /// Edit-phase prepass: input-derived graph mutations for the
    /// already-settled active graph.
    pub(crate) fn prepass(&mut self, ui: &mut Ui, scene: &Scene, out: &mut Vec<Intent>) {
        self.graph_ui.prepass(ui, scene, out);
    }

    pub(crate) fn frame(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        prefs: &mut Preferences,
        doc: &Document,
        out: &mut Vec<Intent>,
    ) -> Option<AppCommand> {
        let mut command = None;
        // The menu bar rides its own chrome band; each pane below carries
        // its own tab strip, and the dock tree fills the space between
        // menu and status bar.
        let chrome = ctx.theme.colors.chrome_fill;
        Panel::vstack()
            .auto_id()
            .size((Sizing::FILL, Sizing::FILL))
            .show(ui, |ui| {
                Panel::hstack()
                    .id_salt("chrome_row")
                    .size((Sizing::FILL, Sizing::Hug))
                    .child_align(Align::v(VAlign::Bottom))
                    .background(Background {
                        fill: chrome.into(),
                        ..Default::default()
                    })
                    .show(ui, |ui| {
                        command = menu_bar::show(ui);
                    });
                let mut walk = DockWalk {
                    ctx,
                    scene,
                    prefs,
                    doc,
                    out,
                    command: &mut command,
                };
                self.render_dock(ui, &mut walk, DockLayout::ROOT, DockPath::ROOT);
                // Bottom chrome: the cache-memory readout, below the panes.
                status_bar::show(ui, ctx);
            });

        if take(&mut self.first_frame) {
            ui.request_relayout();
        }
        command
    }

    /// Recursive walk of the dock tree: a split renders as an aperture
    /// `Splitter` (ratio changes surface as `DockIntent::SetRatio`), a
    /// group as its strip + the active tab's view.
    fn render_dock(&mut self, ui: &mut Ui, w: &mut DockWalk<'_>, idx: NodeIdx, path: DockPath) {
        // Copy the `&Document` out of the walk so the node borrow lives
        // off the document directly, leaving `w` free for the recursion.
        let doc = w.doc;
        match doc.layout.node(idx) {
            DockNode::Group(group) => self.render_group(ui, w, group),
            DockNode::Split(split) => {
                let DockSplit {
                    dir,
                    ratio,
                    first,
                    second,
                } = *split;
                let mut live_ratio = ratio;
                let splitter = match dir {
                    SplitDir::Row => Splitter::horizontal(&mut live_ratio),
                    SplitDir::Column => Splitter::vertical(&mut live_ratio),
                };
                splitter
                    .id(splitter_wid(path))
                    .min_pane(MIN_PANE)
                    .show(ui, |ui, half| {
                        let (child, child_path) = match half {
                            SplitHalf::First => (first, path.first()),
                            SplitHalf::Second => (second, path.second()),
                        };
                        self.render_dock(ui, w, child, child_path);
                    });
                // The widget wrote the divider drag into `live_ratio`; the
                // layout itself only changes through the recorded intent
                // (drained post-record, coalescing per divider).
                if live_ratio != ratio {
                    w.out.push(Intent::Dock(DockIntent::SetRatio {
                        split: path,
                        ratio: live_ratio,
                    }));
                }
            }
        }
    }

    /// One pane: the group's tab strip over its active tab's view.
    fn render_group(&mut self, ui: &mut Ui, w: &mut DockWalk<'_>, group: &TabGroup) {
        let labels = tab_labels(w.doc, group);
        let focused = w.doc.layout.focused == group.id;
        Panel::vstack()
            .id(pane_wid(group.id))
            .size((Sizing::FILL, Sizing::FILL))
            .show(ui, |ui| {
                tab_bar::show(ui, w.ctx.theme, group, &labels, focused, w.out);
                match group.active_tab() {
                    TabRef::Graph(_) => {
                        // Overlay the run/cancel toggle on the canvas's
                        // top-left corner; it hit-tests above the canvas, so a
                        // click on it never starts a pan. Graph tabs live only
                        // in the primary group, so the single canvas scope
                        // can't be recorded twice.
                        Panel::zstack()
                            .id_salt("graph_overlay")
                            .size((Sizing::FILL, Sizing::FILL))
                            .show(ui, |ui| {
                                self.graph_ui.frame(ui, w.ctx, w.scene, w.out, w.command);
                                if let Some(c) = graph_toolbar::show(
                                    ui,
                                    w.ctx,
                                    w.scene,
                                    &self.graph_ui.geometry,
                                    w.out,
                                ) {
                                    *w.command = Some(c);
                                }
                            });
                    }
                    TabRef::Preferences => {
                        if let Some(c) = preferences_view::show(ui, w.ctx.theme, w.prefs) {
                            *w.command = Some(c);
                        }
                    }
                    TabRef::ImageViewer(port) => self.viewer_mut(port).show(ui, w.ctx.theme),
                }
            });
    }

    /// Drop transient input bookkeeping (drag anchors, in-flight
    /// connection) when the active tab changes so a gesture started on
    /// one graph can't bleed into another. Keeps `CanvasGeometry`'s offset
    /// cache so the newly-shown graph's connections render immediately.
    pub(crate) fn reset_transient(&mut self) {
        self.graph_ui.clear_gestures();
    }

    /// The viewer pane for `port`, created empty on first access — a
    /// restored/undo-reopened tab has no state yet; it shows the hint
    /// until the after-run refresh fills it.
    pub(crate) fn viewer_mut(&mut self, port: PortRef) -> &mut ImageViewer {
        self.image_viewers
            .entry(port)
            .or_insert_with(|| ImageViewer::new(port))
    }
}

/// Project one group's tabs into the strip's per-tab labels — the label
/// text is the one thing the strip needs the `Document` for. Lives here
/// (not in `tab_bar`) so the strip stays document-agnostic — same split
/// as `Scene` for the canvas.
fn tab_labels(doc: &Document, group: &TabGroup) -> Vec<TabLabel> {
    group
        .tabs
        .iter()
        .map(|&tab| {
            let text = match tab {
                TabRef::Graph(GraphRef::Main) => "main".into(),
                TabRef::Graph(GraphRef::Local(id)) => doc
                    .graph
                    .subgraphs
                    .by_key(&id)
                    .map(|d| d.name.as_str())
                    .unwrap_or("subgraph")
                    .into(),
                TabRef::Preferences => "preferences".into(),
                TabRef::ImageViewer(port) => image_viewer::port_label(doc, port).into(),
            };
            TabLabel { tab, text }
        })
        .collect()
}

impl Default for MainWindow {
    fn default() -> Self {
        Self {
            graph_ui: GraphUI::default(),
            image_viewers: HashMap::new(),
            first_frame: true,
        }
    }
}
