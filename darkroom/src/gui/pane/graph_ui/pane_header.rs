//! The header over a compound's pane: the path of names from the main graph down to it, each a
//! link to that graph's pane, and the count of nodes that place it once more than one does.
//!
//! - **The header is the path the pane shows the compound through**, as Blender's node editor
//!   and ComfyUI's navigation bar show where a group was entered from. A pane shown through no
//!   instance names the main graph and the compound alone.
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
use scenarium::{CompoundId, GraphId, InstancePath};

use crate::core::document::Document;
use crate::core::document::compound_side::ReadOnly;
use crate::core::document::tab_ref::TabRef;
use crate::core::edit::compound_intent::{CompoundIntent, Update};
use crate::core::edit::make_unique::MakeUnique;
use crate::core::shared_library::{LibraryEntry, SyncState};
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::requests::Requests;
use crate::gui::frame::stage::Stage;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::node_ui::header::read_only_icon;
use crate::gui::theme::Theme;
use crate::gui::widgets::icon_button::IconButton;
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
    /// The pane of an enclosing compound, through the instances down to it.
    Enter {
        compound: CompoundId,
        through: InstancePath,
    },
    /// The pane the header sits on: no link.
    Here,
}

/// Hand `visit` each name on the path down to `compound`'s pane, the main graph first and the
/// compound last: the enclosing graphs come from its side's `through`.
pub(crate) fn for_each_crumb<'a>(
    document: &'a Document,
    compound: CompoundId,
    mut visit: impl FnMut(Crumb<'a>),
) {
    let graphs = &document.graphs;
    let name = |id: CompoundId| {
        graphs
            .compound(id)
            .expect("a path names the compounds it passes")
            .name
            .as_str()
    };
    visit(Crumb {
        name: "Main",
        target: CrumbTarget::Main,
    });
    if let Some(&through) = document.through(GraphId::Compound(compound)) {
        let ids = through.ids();
        for len in 1..ids.len() {
            let prefix = InstancePath::new(&ids[..len]);
            let GraphId::Compound(enclosing) = graphs
                .graph_at(prefix)
                .expect("a side's through names instances")
            else {
                unreachable!("a path past the main graph ends in a compound");
            };
            visit(Crumb {
                name: name(enclosing),
                target: CrumbTarget::Enter {
                    compound: enclosing,
                    through: prefix,
                },
            });
        }
    }
    visit(Crumb {
        name: name(compound),
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

/// A compound's standing against the library, as of a document content revision and a library
/// scan: a digest walks the whole definition, so it is not worked out each frame.
#[derive(Debug, Clone, Copy)]
struct SyncCache {
    compound: CompoundId,
    content: u64,
    scan: u64,
    state: Option<SyncState>,
}

impl PaneHeader {
    /// Where `compound` of the pane's document stands against the library, reusing the last
    /// answer while neither changed; `None` when the library lists no one entry under its id.
    fn sync(&mut self, graph_ctx: GraphCtx<'_>, compound: CompoundId) -> Option<SyncState> {
        let (content, scan) = (
            graph_ctx.open().content_revision(),
            graph_ctx.shared().generation(),
        );
        if let Some(cache) = self.sync
            && (cache.compound, cache.content, cache.scan) == (compound, content, scan)
        {
            return cache.state;
        }
        let document = graph_ctx.document();
        let state = graph_ctx.shared().entry(compound).map(|entry| {
            SyncState::of(
                document.sides[&compound].synced,
                document.graphs.digest(compound),
                entry.digest,
            )
        });
        self.sync = Some(SyncCache {
            compound,
            content,
            scan,
            state,
        });
        state
    }

    /// The header over a compound's pane, at its top edge's middle; nothing over the main
    /// graph's. A click on a name enters that graph's pane.
    pub(super) fn show(&mut self, ui: &mut Ui, stage: &mut Stage<'_, GraphCtx<'_>>) {
        let GraphId::Compound(compound) = stage.ctx.graph() else {
            return;
        };
        let sync = self.sync(stage.ctx, compound);
        show(ui, stage, compound, sync, &mut self.publish_asked);
    }
}

/// The header of `compound`'s pane, standing `sync` against the library.
fn show(
    ui: &mut Ui,
    stage: &mut Stage<'_, GraphCtx<'_>>,
    compound: CompoundId,
    sync: Option<SyncState>,
    publish_asked: &mut bool,
) {
    let (graph_ctx, out) = (stage.ctx, &mut *stage.out);
    let theme = graph_ctx.theme();
    let document = graph_ctx.document();
    let uses = document.graphs.uses(compound).count();
    let panel = Panel::hstack()
        .id_salt("pane_header")
        .align(Align::TOP)
        .child_align(Align::CENTER)
        .margin(Spacing::new(0.0, theme.space.sm, 0.0, 0.0));
    pill::dress(panel, theme).show(ui, |ui| {
        let muted = support::muted_text(ui, theme, theme.font.body);
        let mut idx = 0_usize;
        for_each_crumb(document, compound, |crumb| {
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
                    if Link::new(crumb_wid(compound, idx), crumb.name).show(ui, theme) {
                        out.push_view(DockOperation::OpenTab {
                            tab: TabRef::Graph(GraphId::Main),
                        });
                    }
                }
                // Keyed by the pane, not the graph it leads to: two panes inside one enclosing
                // compound each show a link to it.
                CrumbTarget::Enter {
                    compound: enclosing,
                    through,
                } => {
                    if Link::new(crumb_wid(compound, idx), crumb.name).show(ui, theme) {
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
            if let Some(&through) = document.through(GraphId::Compound(compound))
                && let (Some(instance), Some(parent)) = (through.instance(), through.parent())
                && let Some(holder) = document.graphs.graph_at(parent)
                && Link::new(make_unique_wid(compound), "Make Unique").show(ui, theme)
                && let Some(unique) = MakeUnique::plan(document, holder, &[instance])
            {
                let copy = unique.copy.id;
                unique.queue(out.document());
                out.push_enter(copy, Some(through));
            }
        }
        let side = &document.sides[&compound];
        if side.follows {
            read_only_mark(ui, theme, compound, ReadOnly::Follows);
            Text::new("follows the library").style(&muted).show(ui);
            if Link::new(library_wid(compound, "make_local"), "Make Local")
                .tooltip("Stop following the library, and edit this copy")
                .show(ui, theme)
            {
                out.push_compound(CompoundIntent::SetFollows {
                    id: compound,
                    follows: false,
                });
            }
        } else if let Some(state) = sync
            && let Some(entry) = graph_ctx.shared().entry(compound)
        {
            library_row(ui, graph_ctx, out, compound, entry, state, publish_asked);
            if matches!(state, SyncState::InSync | SyncState::UpdateAvailable)
                && Link::new(library_wid(compound, "follow"), "Follow")
                    .tooltip("Keep this copy as the library's, read-only, updated as it changes")
                    .show(ui, theme)
            {
                out.push_compound(CompoundIntent::SetFollows {
                    id: compound,
                    follows: true,
                });
            }
        }
        let (label, tooltip) = if side.locked {
            read_only_mark(ui, theme, compound, ReadOnly::Locked);
            Text::new("locked").style(&muted).show(ui);
            ("Unlock", "Allow edits of this compound again")
        } else {
            (
                "Lock",
                "Refuse edits of this compound, in every copy, until it is unlocked",
            )
        };
        if Link::new(library_wid(compound, "lock"), label)
            .tooltip(tooltip)
            .show(ui, theme)
        {
            out.push_compound(CompoundIntent::SetLocked {
                id: compound,
                locked: !side.locked,
            });
        }
    });
}

/// The glyph an instance's compound mark wears for the read-only state the header names beside
/// it, so the pane and its instances say it alike.
fn read_only_mark(ui: &mut Ui, theme: &Theme, compound: CompoundId, why: ReadOnly) {
    let tooltip = match why {
        ReadOnly::Locked => "Locked: its graph and interface take no edit",
        ReadOnly::Follows => "Follows the library: its graph and interface take no edit",
    };
    IconButton::mark(
        read_only_icon(why),
        library_wid(compound, "read_only"),
        theme.badge.kind,
        tooltip,
    )
    .show(ui, theme);
}

/// The header's library part: where the compound stands, an Update that puts the library's
/// definition in place, and a Publish that asks before it writes the library file.
fn library_row(
    ui: &mut Ui,
    graph_ctx: GraphCtx<'_>,
    out: &mut Requests,
    compound: CompoundId,
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
    if Link::new(library_wid(compound, "update"), "Update")
        .tooltip("Put the library's definition in place of this one")
        .show(ui, theme)
    {
        out.push_compound(CompoundIntent::Update(Box::new(Update {
            id: compound,
            carried: shared.carried(entry),
            manifest: shared.manifest(entry).clone(),
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
        *publish_asked = Link::new(library_wid(compound, "publish"), "Publish")
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
    if Link::new(library_wid(compound, "publish_yes"), "Publish").show(ui, theme) {
        out.push_app(AppCommand::Publish { compound });
        *publish_asked = false;
    }
    if Link::new(library_wid(compound, "publish_no"), "Cancel").show(ui, theme) {
        *publish_asked = false;
    }
}

/// The library part's `part` in the header over `compound`'s pane.
fn library_wid(compound: CompoundId, part: &'static str) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.pane_header.library", compound, part))
}

/// The Make Unique link in the header over `compound`'s pane.
fn make_unique_wid(compound: CompoundId) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.pane_header.make_unique", compound))
}

/// The `idx`th name's link in the header over `compound`'s pane.
fn crumb_wid(compound: CompoundId, idx: usize) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.pane_header.crumb", compound, idx))
}

#[cfg(test)]
mod tests {
    use scenarium::{Compound, CompoundId, GraphId, InstancePath, Interface, Node, NodeId};

    use crate::core::document::Document;
    use crate::core::document::internals::{DocumentFixture, insert_compound};
    use crate::gui::pane::graph_ui::pane_header::{Crumb, CrumbTarget, for_each_crumb};

    /// The path down to a compound two levels in names the main graph, the enclosing compound
    /// through the outer instance, and the compound itself; shown through no instance it names
    /// the main graph and the compound alone.
    #[test]
    fn the_header_names_each_graph_down_to_the_pane() {
        let mut fixture = DocumentFixture::default();
        let [inner, outer] = [1, 2].map(CompoundId::from_u128);
        insert_compound(
            &mut fixture.document,
            inner,
            Compound::new("Inner", Interface::default()),
        );
        let mut enclosing = Compound::new("Outer", Interface::default());
        let inner_instance = NodeId::from_u128(20);
        let placed = Node::instance(inner);
        enclosing.graph.insert(inner_instance, placed);
        insert_compound(&mut fixture.document, outer, enclosing);
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
                        compound: outer,
                        through: InstancePath::new(&[outer_instance]),
                    }
                ),
                here,
            ]
        );
        assert_eq!(document.active, GraphId::Compound(inner));
    }
}
