//! The header over a subgraph's pane: the path of names from the main graph down to it, each a
//! link to that graph's pane, and the count of nodes that place it once more than one does.
//!
//! - **The header is the path the pane shows the subgraph through**, as Blender's node editor
//!   and ComfyUI's navigation bar show where a group was entered from. A pane shown through no
//!   instance names the main graph and the subgraph alone.
//! - **The count of uses shows once it passes one**: an edit here changes every one of them, and
//!   Grasshopper's users report "no visual sign of entanglement"
//!   ([forum](https://discourse.mcneel.com/t/cluster-disentangled-without-notice/57955)).
//! - **The count of uses sits here, not on the instance**, whose header has no room for it beside
//!   its title and marks; Make Unique stands beside it.
//! - **Publish asks in the header**, naming the file it writes and, when the library changed too,
//!   that it overwrites those changes: one click arms it, a second writes. Refused: a modal dialog
//!   for one yes or no beside the link that asked it.

use palantir::{
    Align, Configure, DockOperation, FontWeight, Panel, Spacing, Text, TextStyle, Ui, WidgetId, fmt,
};
use scenarium::{GraphId, InstancePath, SubgraphId};

use crate::core::document::Document;
use crate::core::document::tab_ref::TabRef;
use crate::core::edit::make_unique::MakeUnique;
use crate::core::edit::subgraph_intent::{SubgraphIntent, Update};
use crate::core::shared_library::{LibraryEntry, SyncState};
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::requests::Requests;
use crate::gui::frame::stage::Stage;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::widgets::link::Link;
use crate::gui::widgets::{pill, support};

/// One name on the path, and where a click on it leads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Crumb<'a> {
    pub(crate) name: &'a str,
    pub(crate) target: CrumbTarget,
}

/// Where a click on a [`Crumb`] leads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CrumbTarget {
    /// The main graph's pane.
    Main,
    /// The pane of an enclosing subgraph, through the instances down to it.
    Enter {
        subgraph: SubgraphId,
        through: InstancePath,
    },
    /// The pane the header sits on: no link.
    Here,
}

/// Hand `visit` each name on the path down to `subgraph`'s pane, the main graph first and the
/// subgraph last: the enclosing graphs come from its side's `through`.
pub(crate) fn for_each_crumb<'a>(
    document: &'a Document,
    subgraph: SubgraphId,
    mut visit: impl FnMut(Crumb<'a>),
) {
    let graphs = &document.graphs;
    let name = |id: SubgraphId| {
        graphs
            .subgraph(id)
            .expect("a path names the subgraphs it passes")
            .name
            .as_str()
    };
    visit(Crumb {
        name: "Main",
        target: CrumbTarget::Main,
    });
    if let Some(&through) = document.through(GraphId::Subgraph(subgraph)) {
        let ids = through.ids();
        for len in 1..ids.len() {
            let prefix = InstancePath::new(&ids[..len]);
            let GraphId::Subgraph(enclosing) = graphs
                .graph_at(prefix)
                .expect("a side's through names instances")
            else {
                unreachable!("a path past the main graph ends in a subgraph");
            };
            visit(Crumb {
                name: name(enclosing),
                target: CrumbTarget::Enter {
                    subgraph: enclosing,
                    through: prefix,
                },
            });
        }
    }
    visit(Crumb {
        name: name(subgraph),
        target: CrumbTarget::Here,
    });
}

/// What the header keeps across frames: its standing against the library, worked out when the
/// document or the library changed, and whether a publish waits on its confirmation.
#[derive(Debug, Default)]
pub(crate) struct PaneHeader {
    sync: Option<SyncCache>,
    publish_asked: bool,
}

/// A subgraph's standing against the library, as of a document content revision and a library
/// scan: a digest walks the whole definition, so it is not worked out each frame.
#[derive(Debug, Clone, Copy)]
struct SyncCache {
    subgraph: SubgraphId,
    content: u64,
    scan: u64,
    state: Option<SyncState>,
}

impl PaneHeader {
    /// Where `subgraph` of the pane's document stands against the library, reusing the last
    /// answer while neither changed; `None` when the library lists no one entry under its id.
    fn sync(&mut self, graph_ctx: GraphCtx<'_>, subgraph: SubgraphId) -> Option<SyncState> {
        let (content, scan) = (
            graph_ctx.open().content_revision(),
            graph_ctx.shared().generation(),
        );
        if let Some(cache) = self.sync
            && (cache.subgraph, cache.content, cache.scan) == (subgraph, content, scan)
        {
            return cache.state;
        }
        let document = graph_ctx.document();
        let state = graph_ctx.shared().entry(subgraph).map(|entry| {
            SyncState::of(
                document.sides[&subgraph].synced,
                document.graphs.digest(subgraph),
                entry.digest,
            )
        });
        self.sync = Some(SyncCache {
            subgraph,
            content,
            scan,
            state,
        });
        state
    }

    /// The header over a subgraph's pane, at its top edge's middle; nothing over the main
    /// graph's. A click on a name enters that graph's pane.
    pub(super) fn show(&mut self, ui: &mut Ui, stage: &mut Stage<'_, GraphCtx<'_>>) {
        let GraphId::Subgraph(subgraph) = stage.ctx.graph() else {
            return;
        };
        let sync = self.sync(stage.ctx, subgraph);
        show(ui, stage, subgraph, sync, &mut self.publish_asked);
    }
}

/// The header of `subgraph`'s pane, standing `sync` against the library.
fn show(
    ui: &mut Ui,
    stage: &mut Stage<'_, GraphCtx<'_>>,
    subgraph: SubgraphId,
    sync: Option<SyncState>,
    publish_asked: &mut bool,
) {
    let (graph_ctx, out) = (stage.ctx, &mut *stage.out);
    let theme = graph_ctx.theme();
    let document = graph_ctx.document();
    let uses = document.graphs.uses(subgraph).count();
    let panel = Panel::hstack()
        .id_salt("pane_header")
        .align(Align::TOP)
        .child_align(Align::CENTER)
        .margin(Spacing::new(0.0, theme.space.sm, 0.0, 0.0));
    pill::dress(panel, theme).show(ui, |ui| {
        let muted = support::muted_text(ui, theme, theme.font.body);
        let mut idx = 0_usize;
        for_each_crumb(document, subgraph, |crumb| {
            if idx > 0 {
                Text::new("\u{203a}").style(&muted).show(ui);
            }
            match crumb.target {
                CrumbTarget::Here => {
                    let bold = TextStyle {
                        weight: FontWeight::BOLD,
                        ..support::body_text(ui, theme)
                    };
                    Text::new(crumb.name).style(&bold).show(ui);
                }
                CrumbTarget::Main => {
                    if Link::new(crumb_wid(subgraph, idx), crumb.name).show(ui, theme) {
                        out.push_view(DockOperation::OpenTab {
                            tab: TabRef::Graph(GraphId::Main),
                        });
                    }
                }
                // Keyed by the pane, not the graph it leads to: two panes inside one enclosing
                // subgraph each show a link to it.
                CrumbTarget::Enter {
                    subgraph: enclosing,
                    through,
                } => {
                    if Link::new(crumb_wid(subgraph, idx), crumb.name).show(ui, theme) {
                        out.push_enter(enclosing, Some(through));
                    }
                }
            }
            idx += 1;
        });
        if uses > 1 {
            Text::new(fmt!(ui, "\u{b7} {uses} uses"))
                .style(&muted)
                .show(ui);
            // The instance the pane is shown through takes a copy of its own, and the pane goes
            // on showing it through that instance.
            // Planned on the click alone, as a plan copies the whole definition.
            if let Some(&through) = document.through(GraphId::Subgraph(subgraph))
                && let (Some(instance), Some(parent)) = (through.instance(), through.parent())
                && let Some(holder) = document.graphs.graph_at(parent)
                && Link::new(make_unique_wid(subgraph), "Make Unique").show(ui, theme)
                && let Some(unique) = MakeUnique::plan(document, holder, &[instance])
            {
                let copy = unique.copy.id;
                unique.queue(out.document());
                out.push_enter(copy, Some(through));
            }
        }
        if let Some(state) = sync
            && let Some(entry) = graph_ctx.shared().entry(subgraph)
        {
            library_row(ui, graph_ctx, out, subgraph, entry, state, publish_asked);
        }
    });
}

/// The header's library part: where the subgraph stands, an Update that puts the library's
/// definition in place, and a Publish that asks before it writes the library file.
fn library_row(
    ui: &mut Ui,
    graph_ctx: GraphCtx<'_>,
    out: &mut Requests,
    subgraph: SubgraphId,
    entry: &LibraryEntry,
    state: SyncState,
    publish_asked: &mut bool,
) {
    let theme = graph_ctx.theme();
    let shared = graph_ctx.shared();
    let muted = support::muted_text(ui, theme, theme.font.body);
    Text::new(fmt!(ui, "\u{b7} {}", state.label()))
        .style(&muted)
        .show(ui);
    if state == SyncState::InSync {
        *publish_asked = false;
        return;
    }
    if Link::new(library_wid(subgraph, "update"), "Update")
        .tooltip("Put the library's definition in place of this one")
        .show(ui, theme)
    {
        out.push_subgraph(SubgraphIntent::Update(Box::new(Update {
            id: subgraph,
            carried: shared.carried(entry),
        })));
    }
    if state == SyncState::UpdateAvailable {
        return;
    }
    let file = shared.path(entry).file_name().map_or_else(
        || shared.path(entry).display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    if !*publish_asked {
        *publish_asked = Link::new(library_wid(subgraph, "publish"), "Publish")
            .tooltip("Write this definition into the library file")
            .show(ui, theme);
        return;
    }
    // Asks first, naming what the write replaces.
    let question = match state {
        SyncState::ChangedInBoth => fmt!(ui, "overwrite the library's changes in {file}?"),
        _ => fmt!(ui, "write it into {file}?"),
    };
    Text::new(question).style(&muted).show(ui);
    if Link::new(library_wid(subgraph, "publish_yes"), "Publish").show(ui, theme) {
        out.push_app(AppCommand::Publish { subgraph });
        *publish_asked = false;
    }
    if Link::new(library_wid(subgraph, "publish_no"), "Cancel").show(ui, theme) {
        *publish_asked = false;
    }
}

/// The library part's `part` in the header over `subgraph`'s pane.
fn library_wid(subgraph: SubgraphId, part: &'static str) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.pane_header.library", subgraph, part))
}

/// The Make Unique link in the header over `subgraph`'s pane.
fn make_unique_wid(subgraph: SubgraphId) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.pane_header.make_unique", subgraph))
}

/// The `idx`th name's link in the header over `subgraph`'s pane.
fn crumb_wid(subgraph: SubgraphId, idx: usize) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.pane_header.crumb", subgraph, idx))
}

#[cfg(test)]
mod tests {
    use scenarium::{GraphId, InstancePath, Interface, Node, NodeId, Subgraph, SubgraphId};

    use crate::core::document::Document;
    use crate::core::document::internals::{DocumentFixture, insert_subgraph};
    use crate::gui::pane::graph_ui::pane_header::{Crumb, CrumbTarget, for_each_crumb};

    /// The path down to a subgraph two levels in names the main graph, the enclosing subgraph
    /// through the outer instance, and the subgraph itself; shown through no instance it names
    /// the main graph and the subgraph alone.
    #[test]
    fn the_header_names_each_graph_down_to_the_pane() {
        let mut fixture = DocumentFixture::default();
        let [inner, outer] = [1, 2].map(SubgraphId::from_u128);
        insert_subgraph(
            &mut fixture.document,
            inner,
            Subgraph::new("Inner", Interface::default()),
        );
        let mut enclosing = Subgraph::new("Outer", Interface::default());
        let inner_instance = NodeId::from_u128(20);
        let placed = Node::instance(inner);
        enclosing.graph.insert(inner_instance, placed);
        insert_subgraph(&mut fixture.document, outer, enclosing);
        let outer_instance = fixture.add_instance(outer);
        let mut document = fixture.document;
        let crumbs = |document: &Document| {
            let mut crumbs = Vec::new();
            for_each_crumb(document, inner, |Crumb { name, target }| {
                crumbs.push((name.to_string(), target));
            });
            crumbs
        };
        let main = ("Main".to_string(), CrumbTarget::Main);
        let here = ("Inner".to_string(), CrumbTarget::Here);

        assert_eq!(crumbs(&document), [main.clone(), here.clone()]);
        document.enter(
            inner,
            Some(InstancePath::new(&[outer_instance, inner_instance])),
        );
        assert_eq!(
            crumbs(&document),
            [
                main,
                (
                    "Outer".to_string(),
                    CrumbTarget::Enter {
                        subgraph: outer,
                        through: InstancePath::new(&[outer_instance]),
                    }
                ),
                here,
            ]
        );
        assert_eq!(document.active, GraphId::Subgraph(inner));
    }
}
