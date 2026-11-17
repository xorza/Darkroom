//! [`Command`]: every action the editor offers by name.
//!
//! # Decisions
//!
//! - **One table.** A menu row, a toolbar chip, a chord and the node menu read a command's
//!   label, its chord and whether it is available here, and raise it through [`Command::raise`],
//!   so no two surfaces name or build one action apart. As Qt's `QAction` represents a command
//!   once for its menu, toolbar and shortcut ([QAction](https://doc.qt.io/qt-6/qaction.html)).
//!   Refused: a trait object a command, as the set is closed; a table a surface, which drifted
//!   to Delete beside Remove.
//! - **A command acts on the active graph and its selection**, read through the [`GraphCtx`] its
//!   caller hands it: the window's for the menu bar and the chords, the pane's own for a node's
//!   menu.
//! - **One label and one chord a command.** Refused: Ctrl+Y beside Ctrl+Shift+Z. Delete keeps
//!   Backspace beside it, read with it: a laptop keyboard may hold no Delete key.
//! - **The camera's commands are Sketch's**, Zoom to 100%, to Fit and to Selection on Ctrl+0, 1
//!   and 2 ([shortcuts](https://go.sketch.com/docs/shortcuts/mac)): Ctrl+digit is an
//!   accelerator, so it fires while a field has the focus and hangs on no keyboard layout.
//!   Refused: Figma's Shift+1 and Shift+2, which type a character.
//! - **Add Node is Shift+A**, Blender's. Refused: Tab, Houdini's and Nuke's, which palantir
//!   classes as focus travel.

pub(crate) mod menu;

use std::collections::BTreeSet;

use common::ErrorChain;
use palantir::{DockOperation, Key, Shortcut, ShortcutMods, TabGroup, Ui};
use scenarium::{Boundary, CompoundId, GraphId, Node, NodeId, ProgramId};

use crate::core::document::item_id::ItemId;
use crate::core::document::pane_kind::PaneKind;
use crate::core::document::tab_ref::TabRef;
use crate::core::edit::collapse;
use crate::core::edit::compound_intent::CompoundIntent;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::incoming_wires::IncomingWires;
use crate::core::edit::make_unique::MakeUnique;
use crate::core::io::preferences::run_mode::RunMode;
use crate::gui::frame::app_command::{AppCommand, FileCommand, RunCommand, ViewCommand};
use crate::gui::frame::clip_commands;
use crate::gui::frame::framing::{Framing, ZoomStep};
use crate::gui::frame::pane_ask::PaneAsk;
use crate::gui::frame::requests::Requests;
use crate::gui::graph_ctx::GraphCtx;

/// Every action the editor offers by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Command {
    NewDocument,
    OpenDocument,
    SaveDocument,
    SaveDocumentAs,
    PackResources,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Duplicate,
    DuplicateWithWires,
    Delete,
    SelectAll,
    Rename,
    ZoomIn,
    ZoomOut,
    ZoomToActual,
    ZoomToFit,
    ZoomToSelection,
    ShowPane(PaneKind),
    CloseTab,
    CopyViewImage,
    SaveViewImage,
    Run,
    CancelRun,
    RunToSelection,
    ToggleEvents,
    ToggleRunOnChange,
    AddNode,
    ReplaceNode,
    OpenCompound,
    CollapseToCompound,
    ExpandCompound,
    MakeUnique,
    MakeLocal,
    ToggleLock,
    ExportCompound,
    SelectGhosts,
    RemoveGhosts,
    RemoveUnusedCompounds,
}

/// What a command is called, the chord that raises it and what it needs on screen, the same
/// wherever it shows.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CommandSpec {
    /// Title case; ends in an ellipsis when the command asks before it acts.
    pub(crate) label: &'static str,
    pub(crate) keys: Option<Shortcut>,
    pub(crate) scope: Scope,
}

/// What a command needs on screen to be raised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    /// Nothing: the file's commands, the history, the run, the panes.
    Window,
    /// A pane that shows the graph it acts on: the commands that edit or frame a graph, so a
    /// Delete removes nothing nobody is looking at, by its chord and by its row alike.
    Graph,
}

/// Whether a command can be raised now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Availability {
    /// Raising it would change nothing: a greyed row, a chord that falls through.
    Unavailable,
    Available,
    /// In effect, and shown checked: a toggle that is on, a pane that is open.
    On,
}

impl Availability {
    /// `Available` when `available`, else `Unavailable`.
    const fn when(available: bool) -> Self {
        if available {
            Self::Available
        } else {
            Self::Unavailable
        }
    }

    /// `On` when `on`, else `Available`: a toggle's two states.
    const fn toggled(on: bool) -> Self {
        if on { Self::On } else { Self::Available }
    }
}

const CTRL_SHIFT: ShortcutMods = ShortcutMods::CTRL.union(ShortcutMods::SHIFT);

impl Command {
    /// Every command, a pane's once a kind, in the enum's order: what the chord read and the
    /// reference walk.
    pub(crate) const ALL: [Self; 45] = [
        Self::NewDocument,
        Self::OpenDocument,
        Self::SaveDocument,
        Self::SaveDocumentAs,
        Self::PackResources,
        Self::Quit,
        Self::Undo,
        Self::Redo,
        Self::Cut,
        Self::Copy,
        Self::Paste,
        Self::Duplicate,
        Self::DuplicateWithWires,
        Self::Delete,
        Self::SelectAll,
        Self::Rename,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::ZoomToActual,
        Self::ZoomToFit,
        Self::ZoomToSelection,
        Self::ShowPane(PaneKind::Viewer),
        Self::ShowPane(PaneKind::Inspector),
        Self::ShowPane(PaneKind::Preferences),
        Self::ShowPane(PaneKind::Reference),
        Self::CloseTab,
        Self::CopyViewImage,
        Self::SaveViewImage,
        Self::Run,
        Self::CancelRun,
        Self::RunToSelection,
        Self::ToggleEvents,
        Self::ToggleRunOnChange,
        Self::AddNode,
        Self::ReplaceNode,
        Self::OpenCompound,
        Self::CollapseToCompound,
        Self::ExpandCompound,
        Self::MakeUnique,
        Self::MakeLocal,
        Self::ToggleLock,
        Self::ExportCompound,
        Self::SelectGhosts,
        Self::RemoveGhosts,
        Self::RemoveUnusedCompounds,
    ];

    /// The command's label, chord and scope.
    pub(crate) const fn spec(self) -> CommandSpec {
        use Scope::{Graph, Window};
        const fn ctrl(c: char) -> Shortcut {
            Shortcut::ctrl(c)
        }
        const fn ctrl_shift(c: char) -> Shortcut {
            Shortcut::new(CTRL_SHIFT, Key::Char(c))
        }
        const fn key(key: Key) -> Shortcut {
            Shortcut::key(key)
        }
        let (label, keys, scope) = match self {
            Self::NewDocument => ("New", Some(ctrl('N')), Window),
            Self::OpenDocument => ("Open\u{2026}", Some(ctrl('O')), Window),
            Self::SaveDocument => ("Save", Some(ctrl('S')), Window),
            Self::SaveDocumentAs => ("Save As\u{2026}", Some(ctrl_shift('S')), Window),
            Self::PackResources => ("Pack Resources", None, Window),
            // Through the unsaved-changes prompt, as File ▸ Quit: palantir drops winit's default
            // macOS menu, so ⌘Q reaches the editor instead of ending it.
            Self::Quit => ("Quit", Some(ctrl('Q')), Window),
            Self::Undo => ("Undo", Some(ctrl('Z')), Window),
            Self::Redo => ("Redo", Some(ctrl_shift('Z')), Window),
            Self::Cut => ("Cut", Some(ctrl('X')), Graph),
            Self::Copy => ("Copy", Some(ctrl('C')), Graph),
            Self::Paste => ("Paste", Some(ctrl('V')), Graph),
            Self::Duplicate => ("Duplicate", Some(ctrl('D')), Graph),
            Self::DuplicateWithWires => ("Duplicate with Incoming Wires", None, Graph),
            Self::Delete => ("Delete", Some(key(Key::Delete)), Graph),
            Self::SelectAll => ("Select All", Some(ctrl('A')), Graph),
            Self::Rename => ("Rename", Some(key(Key::F2)), Graph),
            Self::ZoomIn => ("Zoom In", Some(ctrl('=')), Graph),
            Self::ZoomOut => ("Zoom Out", Some(ctrl('-')), Graph),
            Self::ZoomToActual => ("Zoom to 100%", Some(ctrl('0')), Graph),
            Self::ZoomToFit => ("Zoom to Fit", Some(ctrl('1')), Graph),
            Self::ZoomToSelection => ("Zoom to Selection", Some(ctrl('2')), Graph),
            Self::ShowPane(PaneKind::Preferences) => {
                (PaneKind::Preferences.label(), Some(ctrl(',')), Window)
            }
            Self::ShowPane(kind) => (kind.label(), None, Window),
            Self::CloseTab => ("Close Tab", Some(ctrl('W')), Window),
            Self::CopyViewImage => ("Copy View as Image", None, Window),
            Self::SaveViewImage => ("Save View as PNG\u{2026}", None, Window),
            Self::Run => ("Run", Some(ctrl('R')), Window),
            Self::CancelRun => ("Cancel Run", None, Window),
            Self::RunToSelection => ("Run to Selected Node", None, Graph),
            Self::ToggleEvents => ("Event Loop", None, Window),
            Self::ToggleRunOnChange => ("Run on Change", None, Window),
            Self::AddNode => (
                "Add Node\u{2026}",
                Some(Shortcut::new(ShortcutMods::SHIFT, Key::Char('A'))),
                Graph,
            ),
            Self::ReplaceNode => ("Replace with\u{2026}", None, Graph),
            Self::OpenCompound => ("Open", None, Graph),
            Self::CollapseToCompound => ("Collapse to Compound", None, Graph),
            Self::ExpandCompound => ("Expand Compound", None, Graph),
            Self::MakeUnique => ("Make Unique", None, Graph),
            Self::MakeLocal => ("Make Local", None, Graph),
            Self::ToggleLock => ("Locked", None, Graph),
            Self::ExportCompound => ("Export Compound\u{2026}", None, Window),
            Self::SelectGhosts => ("Select Ghosts", None, Graph),
            Self::RemoveGhosts => ("Remove Ghosts", None, Graph),
            Self::RemoveUnusedCompounds => ("Remove Unused Compounds", None, Window),
        };
        CommandSpec { label, keys, scope }
    }

    /// The command's keys, when it has a chord: what a menu row and a chip's tooltip show.
    pub(crate) const fn keys(self) -> Option<Shortcut> {
        self.spec().keys
    }

    /// The command's keys, for a test that presses its chord.
    #[cfg(test)]
    pub(crate) const fn chord_keys(self) -> Shortcut {
        self.keys().expect("the command has a chord")
    }

    /// Whether the command's chord is pressed this frame. Read every frame for every command,
    /// pressed or not: the read also watches the chord, and palantir's wake-gate delivers an
    /// off-focus press only for a chord watched the frame before.
    pub(crate) fn chord_pressed(self, ui: &mut Ui) -> bool {
        let Some(keys) = self.keys() else {
            return false;
        };
        let pressed = ui.shortcut_pressed(keys);
        // Backspace removes the selection too: a laptop keyboard may hold no Delete key.
        let backspace = matches!(self, Self::Delete) && ui.shortcut_pressed(Key::Backspace);
        pressed || backspace
    }

    /// Whether the command can be raised against `ctx`, the graph it acts on, and whether it
    /// shows checked. It reads the document and the frame's world alone, never a widget, so a
    /// row drawn in the record and a chord read in the input stage answer alike.
    pub(crate) fn availability(self, ctx: GraphCtx<'_>) -> Availability {
        let document = ctx.document();
        if self.spec().scope == Scope::Graph {
            let shown = TabRef::Graph(ctx.graph());
            if !document.layout.active_tabs().any(|tab| tab == shown) {
                return Availability::Unavailable;
            }
        }
        let activity = ctx.run_state().activity();
        match self {
            Self::NewDocument
            | Self::OpenDocument
            | Self::SaveDocument
            | Self::SaveDocumentAs
            | Self::PackResources
            | Self::Quit
            // The clipboard is read on the raise, as holding a clip it does not fit is a
            // warning worth its words.
            | Self::Paste
            | Self::ZoomIn
            | Self::ZoomOut
            | Self::ZoomToActual
            | Self::ZoomToFit
            | Self::CopyViewImage
            | Self::SaveViewImage
            | Self::AddNode => Availability::Available,
            Self::Undo => Availability::when(ctx.open().can_undo()),
            Self::Redo => Availability::when(ctx.open().can_redo()),
            Self::Cut | Self::Copy | Self::Duplicate | Self::DuplicateWithWires => {
                Availability::when(selects_live_node(ctx))
            }
            Self::Delete => Availability::when(selects_removable(ctx)),
            Self::SelectAll => {
                let view = ctx.view();
                Availability::when(!view.node_placements.is_empty() || !view.ties.is_empty())
            }
            Self::Rename => Availability::when(only_live_node(ctx).is_some()),
            Self::ZoomToSelection => Availability::when(!ctx.selected().is_empty()),
            Self::CollapseToCompound => Availability::when(selects_live_node(ctx)),
            Self::ShowPane(kind) => {
                let tab = kind.tab();
                Availability::toggled(document.layout.all_tabs().any(|open| open == tab))
            }
            Self::CloseTab => Availability::when(closable_tab(ctx).is_some()),
            Self::Run => Availability::when(!activity.is_executing()),
            Self::CancelRun => Availability::when(activity.is_executing()),
            Self::RunToSelection => Availability::when(runnable(ctx).is_some()),
            Self::ToggleEvents => Availability::toggled(activity.event_loop_active()),
            Self::ToggleRunOnChange => {
                Availability::toggled(ctx.run_mode() == RunMode::OnChange)
            }
            Self::ReplaceNode => Availability::when(replaceable(ctx).is_some()),
            Self::OpenCompound | Self::ExpandCompound => {
                Availability::when(only_instance(ctx).is_some())
            }
            // Offered once another node places the compound too: a copy for its every
            // instance is the compound itself.
            Self::MakeUnique => {
                let picked = selected_nodes(ctx);
                Availability::when(
                    MakeUnique::placed(document, ctx.graph(), &picked)
                        .is_some_and(|id| document.graphs.uses(id).count() > picked.len()),
                )
            }
            Self::MakeLocal => Availability::when(
                only_instance(ctx).is_some_and(|instance| document.sides[&instance.compound].follows),
            ),
            Self::ToggleLock => match only_instance(ctx) {
                Some(instance) => {
                    Availability::toggled(document.sides[&instance.compound].locked)
                }
                None => Availability::Unavailable,
            },
            Self::ExportCompound => Availability::when(exported(ctx).is_some()),
            Self::SelectGhosts | Self::RemoveGhosts => {
                let held = ctx.body();
                Availability::when(
                    held.ghost_nodes().len() > 0 || !held.ghost_wiring().is_empty(),
                )
            }
            Self::RemoveUnusedCompounds => {
                Availability::when(!document.unused_compounds().is_empty())
            }
        }
    }

    /// Raise the command against `ctx`, the active graph's: graph edits as intents, the file, the
    /// history, the run and the view as [`AppCommand`]s for `App` to run after the pass, a pane
    /// as the dock operation that shows it. The caller found the command available; one whose
    /// target went since raises nothing.
    pub(crate) fn raise(self, ui: &Ui, ctx: GraphCtx<'_>, out: &mut Requests) {
        let (graph, document, library) = (ctx.graph(), ctx.document(), ctx.library());
        let ask = |out: &mut Requests, ask: PaneAsk| out.push_app(AppCommand::Pane { graph, ask });
        match self {
            Self::NewDocument => out.push_app(AppCommand::File(FileCommand::New)),
            Self::OpenDocument => out.push_app(AppCommand::File(FileCommand::Open)),
            Self::SaveDocument => out.push_app(AppCommand::File(FileCommand::Save)),
            Self::SaveDocumentAs => out.push_app(AppCommand::File(FileCommand::SaveAs)),
            Self::PackResources => out.push_app(AppCommand::File(FileCommand::PackResources)),
            Self::Quit => out.push_app(AppCommand::Quit),
            Self::Undo => out.push_app(AppCommand::Undo),
            Self::Redo => out.push_app(AppCommand::Redo),
            Self::Cut => {
                clip_commands::copy(ui, document, graph, library, ctx.selected());
                out.extend_graph(graph, GraphIntent::remove_selection(ctx.view()));
            }
            Self::Copy => clip_commands::copy(ui, document, graph, library, ctx.selected()),
            Self::Paste => clip_commands::paste(ui, document, graph, library, out),
            // Duplicate drops the wires from producers outside the selection.
            Self::Duplicate => duplicate(ctx, IncomingWires::Drop, out),
            Self::DuplicateWithWires => duplicate(ctx, IncomingWires::Keep, out),
            // One intent a member, batched into one undo entry by the drain.
            Self::Delete => out.extend_graph(graph, GraphIntent::remove_selection(ctx.view())),
            Self::SelectAll => {
                let view = ctx.view();
                let nodes = view.node_placements.keys().copied().map(ItemId::Node);
                let ties = view.ties.keys().copied().map(ItemId::Tie);
                out.push_graph(
                    graph,
                    GraphIntent::SetSelection {
                        to: nodes.chain(ties).collect::<BTreeSet<_>>(),
                    },
                );
            }
            Self::Rename => {
                if let Some(node_id) = only_live_node(ctx) {
                    ask(out, PaneAsk::Rename(node_id));
                }
            }
            Self::ZoomIn => ask(out, PaneAsk::Frame(Framing::Step(ZoomStep::In))),
            Self::ZoomOut => ask(out, PaneAsk::Frame(Framing::Step(ZoomStep::Out))),
            Self::ZoomToActual => ask(out, PaneAsk::Frame(Framing::Actual)),
            Self::ZoomToFit => ask(out, PaneAsk::Frame(Framing::Fit)),
            Self::ZoomToSelection => ask(out, PaneAsk::Frame(Framing::Selection)),
            Self::ShowPane(kind) => out.push_show(kind.tab()),
            Self::CloseTab => {
                if let Some(tab) = closable_tab(ctx) {
                    out.push_view(DockOperation::CloseTab { tab });
                }
            }
            Self::CopyViewImage => out.push_app(AppCommand::View(ViewCommand::CopyImage)),
            Self::SaveViewImage => out.push_app(AppCommand::View(ViewCommand::SaveImage)),
            Self::Run => out.push_app(AppCommand::Run(RunCommand::Once)),
            Self::CancelRun => out.push_app(AppCommand::Run(RunCommand::Cancel)),
            Self::RunToSelection => {
                if let Some(program_id) = runnable(ctx) {
                    out.push_app(AppCommand::Run(RunCommand::Node(program_id)));
                }
            }
            Self::ToggleEvents => {
                let command = if ctx.run_state().activity().event_loop_active() {
                    RunCommand::StopEvents
                } else {
                    RunCommand::StartEvents
                };
                out.push_app(AppCommand::Run(command));
            }
            Self::ToggleRunOnChange => {
                out.push_app(AppCommand::SetRunMode(ctx.run_mode().toggled()));
            }
            Self::AddNode => ask(out, PaneAsk::AddNode),
            Self::ReplaceNode => {
                if let Some(node_id) = replaceable(ctx) {
                    ask(out, PaneAsk::Replace(node_id));
                }
            }
            Self::OpenCompound => {
                if let Some(instance) = only_instance(ctx) {
                    let through = ctx.through().map(|path| path.child(instance.node_id));
                    out.push_enter(instance.compound, through);
                }
            }
            Self::CollapseToCompound => {
                match collapse::collapse(document, graph, library, ctx.selected()) {
                    Some(Ok(plan)) => plan.queue(graph, out.document()),
                    Some(Err(refusal)) => {
                        out.push_app(AppCommand::Warn(ErrorChain::new(&refusal).to_string()));
                    }
                    None => {}
                }
            }
            Self::ExpandCompound => {
                if let Some(instance) = only_instance(ctx) {
                    out.extend_graph(
                        graph,
                        collapse::expand(document, graph, library, instance.node_id),
                    );
                }
            }
            // Planned on the raise alone, as a plan copies the whole definition.
            Self::MakeUnique => {
                if let Some(unique) = MakeUnique::plan(document, graph, &selected_nodes(ctx)) {
                    unique.queue(out.document());
                }
            }
            Self::MakeLocal => {
                if let Some(instance) = only_instance(ctx) {
                    out.push_compound(CompoundIntent::SetFollows {
                        id: instance.compound,
                        follows: false,
                    });
                }
            }
            Self::ToggleLock => {
                if let Some(instance) = only_instance(ctx) {
                    out.push_compound(CompoundIntent::SetLocked {
                        id: instance.compound,
                        locked: !document.sides[&instance.compound].locked,
                    });
                }
            }
            Self::ExportCompound => {
                if let Some(compound) = exported(ctx) {
                    out.push_app(AppCommand::Export { compound });
                }
            }
            Self::SelectGhosts => out.push_graph(graph, GraphIntent::select_ghosts(ctx.body())),
            // One batch, so one undo entry.
            Self::RemoveGhosts => out.extend_graph(graph, GraphIntent::remove_ghosts(ctx.body())),
            // One batch, so one undo entry, outermost first so each removal holds.
            Self::RemoveUnusedCompounds => {
                for id in document.unused_compounds() {
                    out.push_compound(CompoundIntent::Remove { id });
                }
            }
        }
    }
}

/// An instance and the compound it places.
#[derive(Clone, Copy, Debug)]
struct Instance {
    node_id: NodeId,
    compound: CompoundId,
}

/// The nodes the selection holds, ties aside.
fn selected_nodes(ctx: GraphCtx<'_>) -> Vec<NodeId> {
    ctx.selected()
        .iter()
        .filter_map(|item| item.node())
        .collect()
}

/// The one node the selection holds, ties aside, or `None` for none or several.
fn only_node(ctx: GraphCtx<'_>) -> Option<NodeId> {
    let mut nodes = ctx.selected().iter().filter_map(|item| item.node());
    let node_id = nodes.next()?;
    nodes.next().is_none().then_some(node_id)
}

/// [`only_node`], when the graph holds it as a node and not as a ghost.
fn only_live_node(ctx: GraphCtx<'_>) -> Option<NodeId> {
    only_node(ctx).filter(|&node_id| ctx.body().find(node_id).is_some())
}

/// [`only_node`], when it is an instance.
fn only_instance(ctx: GraphCtx<'_>) -> Option<Instance> {
    let node_id = only_node(ctx)?;
    let compound = ctx.body().find(node_id)?.compound()?;
    Some(Instance { node_id, compound })
}

/// Whether the selection holds a node the graph holds live: what a copy or a duplicate takes, as
/// each skips a ghost node.
fn selects_live_node(ctx: GraphCtx<'_>) -> bool {
    ctx.selected()
        .iter()
        .filter_map(|item| item.node())
        .any(|node_id| ctx.body().find(node_id).is_some())
}

/// Whether a Delete would take anything: a tie, a ghost node, or a node that is not a boundary
/// its compound keeps, its `Outputs` node and its last `Inputs` node.
fn selects_removable(ctx: GraphCtx<'_>) -> bool {
    let body = ctx.body();
    let inputs_count = || {
        body.iter()
            .filter(|node| node.boundary() == Some(Boundary::Inputs))
            .count()
    };
    ctx.selected().iter().any(|item| match item.node() {
        None => true,
        Some(node_id) => match body.find(node_id).and_then(Node::boundary) {
            None => true,
            Some(Boundary::Outputs) => false,
            Some(Boundary::Inputs) => inputs_count() > 1,
        },
    })
}

/// The program node a Run to Selected Node seeds: the one selected node, in the instance the
/// pane shows, when it runs.
fn runnable(ctx: GraphCtx<'_>) -> Option<ProgramId> {
    ctx.node(only_node(ctx)?)
        .filter(|node| !node.ghost())?
        .runnable()
}

/// The node a Replace with… takes the place of: the one selected node, a ghost node included,
/// unless it is a boundary node.
fn replaceable(ctx: GraphCtx<'_>) -> Option<NodeId> {
    let node_id = only_node(ctx)?;
    let boundary = ctx.body().find(node_id).and_then(Node::boundary);
    boundary.is_none().then_some(node_id)
}

/// The compound an Export writes: the one instance the selection holds, else the compound the
/// active pane shows.
fn exported(ctx: GraphCtx<'_>) -> Option<CompoundId> {
    only_instance(ctx)
        .map(|instance| instance.compound)
        .or_else(|| match ctx.graph() {
            GraphId::Compound(id) => Some(id),
            GraphId::Main => None,
        })
}

/// The tab a Close Tab closes: the focused group's visible one, unless it is the pinned main
/// graph's.
fn closable_tab(ctx: GraphCtx<'_>) -> Option<TabRef> {
    let layout = &ctx.document().layout;
    let tab = layout.group(layout.focused()).map(TabGroup::active_tab)?;
    (tab != layout.pinned()).then_some(tab)
}

fn duplicate(ctx: GraphCtx<'_>, incoming: IncomingWires, out: &mut Requests) {
    out.extend_graph(
        ctx.graph(),
        GraphIntent::duplicate(ctx.document(), ctx.graph(), ctx.library(), incoming, || {
            NodeId::unique().as_u128()
        }),
    );
}

#[cfg(test)]
mod tests;
