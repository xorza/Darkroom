//! The band floating over the top of every graph pane: at the left the run's controls and the path
//! of names from the main graph down to the pane, with a compound's count of uses and where it
//! stands, on one pill; at the right a compound's actions on a pill, and the zoom on a pill of its
//! own, which opens the camera's commands.
//!
//! - **Pills floating over the canvas**, the viewer's toolbar pills at their alpha, so the tab
//!   strip sits on the pane it names and the canvas runs up to it. As Blender's Region Overlap
//!   draws an editor's toolbar and sidebar over its main region
//!   ([preferences](https://docs.blender.org/manual/en/latest/editors/preferences/interface.html)).
//!   Refused: a band fixed above the canvas, a row of surround that set the tabs apart from their
//!   pane. Cost: the pills cover the nodes under them; between them a press reaches the canvas.
//! - **The path is the one a pane shows its graph through**, as Blender's node editor and
//!   ComfyUI's navigation bar show where a group was entered from: the main graph's pane reads
//!   Main, and a compound's pane shown through no instance names the main graph and the compound
//!   alone. Where the band runs short the pane's name gives way, cut with an ellipsis, and the
//!   controls, the actions and the zoom read whole.
//! - **The count of uses shows once it passes one**: an edit here changes every one of them, and
//!   Grasshopper's users report "no visual sign of entanglement"
//!   ([forum](https://discourse.mcneel.com/t/cluster-disentangled-without-notice/57955)).
//! - **The count of uses sits here, not on the instance**, whose header has no room for it beside
//!   its title and marks.
//! - **A compound's actions are icon chips at the right, its path links at the left**: a link
//!   goes somewhere, a chip acts, and the actions keep their places however long the path runs. A
//!   state an action toggles, locked or following the library, shows as its chip's on state, as
//!   the event loop's does, and in words on the path. Refused: text buttons beside the run chips,
//!   two looks of a control on one pill.
//! - **Publish asks in the band**, naming the file it writes and, when the library changed too,
//!   that it overwrites those changes: one click arms it, a second writes. Refused: a modal dialog
//!   for one yes or no beside the button that asked it.

use std::path::Path;

use common::FloatExt;
use glam::Vec2;
use palantir::{
    Align, Button, ClipMode, Configure, CursorIcon, DockOperation, FontFamily, FontWeight, Menu,
    Panel, Sense, SizeSpec, Sizing, Spacing, Text, TextRun, TextStyle, TextWrap, Ui, VAlign,
    WidgetId, fmt,
};
use scenarium::{CompoundId, GraphId, InstancePath};

use crate::core::document::Document;
use crate::core::document::compound_side::ReadOnly;
use crate::core::document::tab_ref::TabRef;
use crate::core::document::viewport::Viewport;
use crate::core::edit::compound_intent::{CompoundIntent, Update};
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::make_unique::MakeUnique;
use crate::core::io::preferences::run_mode::RunMode;
use crate::core::shared_library::{LibraryEntry, SyncState};
use crate::gui::command::Command;
use crate::gui::command::menu::{self, RUN_MENU, Unavailable, ZOOM_MENU};
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::stage::Stage;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::gesture::pan_zoom;
use crate::gui::pane::graph_ui::node_ui::header::read_only_icon;
use crate::gui::widgets::chip::Chip;
use crate::gui::widgets::format::percent::Percent;
use crate::gui::widgets::icon::Icon;
use crate::gui::widgets::link::Link;
use crate::gui::widgets::pill;
use crate::gui::widgets::support;

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
            .decl
            .name
            .as_str()
    };
    visit(Crumb {
        name: MAIN,
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
    /// The camera where a drag of the zoom control latched, while it is held.
    zoom_scrub: Option<Viewport>,
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

/// Where a compound shared through the library stands: its entry there, and its state against it.
#[derive(Debug, Clone, Copy)]
struct Standing<'a> {
    compound: CompoundId,
    entry: &'a LibraryEntry,
    state: SyncState,
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

    /// A compound pane's path, its count of uses and where it stands: following the library, else
    /// against it, and locked.
    fn compound_path(
        &mut self,
        ui: &mut Ui,
        stage: &mut Stage<'_, GraphCtx<'_>>,
        compound: CompoundId,
    ) {
        let sync = self.sync(stage.ctx, compound);
        let (graph_ctx, out) = (stage.ctx, &mut *stage.out);
        let theme = graph_ctx.theme();
        let document = graph_ctx.document();
        let muted = support::muted_text(ui, theme, theme.font.body);
        let mut idx = 0_usize;
        for_each_crumb(document, compound, |crumb| {
            if idx > 0 {
                Text::new("\u{203a}").style(&muted).show(ui);
            }
            match crumb.target {
                // The pane's own name gives way first where the band runs short, cut with an
                // ellipsis, so what follows it keeps its place.
                CrumbTarget::Here => {
                    let bold = TextStyle {
                        weight: FontWeight::BOLD,
                        ..support::body_text(ui, theme)
                    };
                    Text::new(crumb.name)
                        .style(&bold)
                        .text_wrap(TextWrap::Ellipsis)
                        .show(ui);
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
        let uses = document.graphs.uses(compound).count();
        if uses > 1 {
            Text::new(fmt!(ui, "\u{b7} {uses} uses"))
                .style(&muted)
                .show(ui);
        }
        let side = &document.sides[&compound];
        if side.follows {
            Text::new("\u{b7} follows the library")
                .style(&muted)
                .show(ui);
        } else if let Some(state) = sync {
            Text::new(fmt!(ui, "\u{b7} {}", state.label()))
                .style(&muted)
                .show(ui);
        }
        if side.locked {
            Text::new("\u{b7} locked").style(&muted).show(ui);
        }
    }

    /// A compound pane's actions, standing against the library: Make Unique once another
    /// instance places it; Update, then Publish, which asks before it writes, while it differs from the
    /// library and follows it not; following the library; and the lock.
    fn compound_actions(
        &mut self,
        ui: &mut Ui,
        stage: &mut Stage<'_, GraphCtx<'_>>,
        compound: CompoundId,
    ) {
        let sync = self.sync(stage.ctx, compound);
        let graph_ctx = stage.ctx;
        let theme = graph_ctx.theme();
        let document = graph_ctx.document();
        let shared = graph_ctx.shared();
        // The instance the pane is shown through takes a copy of its own, and the pane goes on showing
        // it through that instance. Planned on the click alone, as a plan copies the whole definition.
        if document.graphs.uses(compound).count() > 1
            && let Some(&through) = document.through(GraphId::Compound(compound))
            && let (Some(instance), Some(parent)) = (through.instance(), through.parent())
            && let Some(holder) = document.graphs.graph_at(parent)
            && Chip::new(
                make_unique_wid(compound),
                "Make Unique \u{2014} give this instance a copy of its own",
            )
            .show_icon(ui, theme, Icon::MakeUnique)
            && let Some(unique) = MakeUnique::plan(document, holder, &[instance])
        {
            let copy = unique.copy.id;
            unique.queue(stage.out.document());
            stage.out.push_enter(copy, Some(through));
        }
        let side = &document.sides[&compound];
        let entry = shared.entry(compound);
        match (side.follows, sync, entry) {
            (false, Some(state), Some(entry)) if state != SyncState::InSync => {
                if Chip::new(
                    library_wid(compound, "update"),
                    "Update \u{2014} put the library's definition in place of this one",
                )
                .show_icon(ui, theme, Icon::Update)
                {
                    stage
                        .out
                        .push_compound(CompoundIntent::Update(Box::new(Update {
                            id: compound,
                            carried: shared.carried(entry),
                            manifest: shared.manifest(entry).clone(),
                        })));
                }
                if state == SyncState::UpdateAvailable {
                    self.publish_asked = false;
                } else {
                    let standing = Standing {
                        compound,
                        entry,
                        state,
                    };
                    self.publish(ui, stage, standing);
                }
            }
            _ => self.publish_asked = false,
        }
        let follows = side.follows;
        let can_follow = matches!(sync, Some(SyncState::InSync | SyncState::UpdateAvailable));
        if follows || can_follow {
            let tooltip = if follows {
                "Follows the library \u{2014} click to make this copy local and edit it"
            } else {
                "Follow the library \u{2014} keep this copy as the library's, read-only, updated as it changes"
            };
            if Chip::new(library_wid(compound, "follow"), tooltip)
                .selected(follows)
                .show_icon(ui, theme, read_only_icon(ReadOnly::Follows))
            {
                stage.out.push_compound(CompoundIntent::SetFollows {
                    id: compound,
                    follows: !follows,
                });
            }
        }
        let tooltip = if side.locked {
            "Locked \u{2014} click to allow edits of this compound again"
        } else {
            "Lock \u{2014} refuse edits of this compound, in every copy, until it is unlocked"
        };
        if Chip::new(library_wid(compound, "lock"), tooltip)
            .selected(side.locked)
            .show_icon(ui, theme, read_only_icon(ReadOnly::Locked))
        {
            stage.out.push_compound(CompoundIntent::SetLocked {
                id: compound,
                locked: !side.locked,
            });
        }
    }

    /// Publish, which a first click arms and a second confirms: armed, the chip shows its on state
    /// beside the question, naming the library file of the compound's `standing` and, when the
    /// library changed too, that the write overwrites those changes, and a check and a cross that
    /// answer it.
    fn publish(
        &mut self,
        ui: &mut Ui,
        stage: &mut Stage<'_, GraphCtx<'_>>,
        standing: Standing<'_>,
    ) {
        let (graph_ctx, out) = (stage.ctx, &mut *stage.out);
        let Standing {
            compound,
            entry,
            state,
        } = standing;
        let publish_asked = &mut self.publish_asked;
        let theme = graph_ctx.theme();
        if Chip::new(
            library_wid(compound, "publish"),
            "Publish \u{2014} write this definition into the library file",
        )
        .selected(*publish_asked)
        .show_icon(ui, theme, Icon::Publish)
        {
            *publish_asked = !*publish_asked;
        }
        if !*publish_asked {
            return;
        }
        let path = graph_ctx.shared().path(entry);
        // Formatted into the frame's arena, so an armed Publish builds no `String` a frame.
        let file = Path::new(path.file_name().unwrap_or(path.as_os_str())).display();
        let question = match state {
            SyncState::ChangedInBoth => fmt!(ui, "overwrite the library's changes in {file}?"),
            _ => fmt!(ui, "write it into {file}?"),
        };
        Text::new(question)
            .style(&support::muted_text(ui, theme, theme.font.body))
            .padding(Spacing::xy(theme.space.xs, 0.0))
            .show(ui);
        if Chip::new(library_wid(compound, "publish_yes"), "Publish").show_icon(
            ui,
            theme,
            Icon::Check,
        ) {
            out.push_app(AppCommand::Publish { compound });
            *publish_asked = false;
        }
        if Chip::new(library_wid(compound, "publish_no"), "Cancel").show_icon(
            ui,
            theme,
            Icon::Cross,
        ) {
            *publish_asked = false;
        }
    }

    /// The band over `stage`'s graph pane. A click on a name enters that graph's pane.
    pub(super) fn show(&mut self, ui: &mut Ui, stage: &mut Stage<'_, GraphCtx<'_>>) {
        let graph = stage.ctx.graph();
        let theme = stage.ctx.theme();
        let margin = theme.space.sm;
        // The right side stands alone, so it never gives way; the controls' pill
        // takes what the band leaves it as of last frame's layout, where the pane's name gives way
        // first and the pill clips what still runs past it. Refused: both in one row, which
        // shares a shortfall among its children and squeezes the zoom too.
        let last_w = |id: WidgetId| ui.response_for(id).layout_rect.map(|rect| rect.size.w);
        let controls_w_max = last_w(band_wid(graph)).map_or(f32::INFINITY, |band_w| {
            let right_w = last_w(right_wid(graph)).unwrap_or(0.0);
            // A margin at each end and one between the two sides.
            (band_w - 3.0 * margin - right_w).max(0.0)
        });
        // A band with no sense of its own: between the pills a press reaches the canvas.
        Panel::zstack()
            .id(band_wid(graph))
            .size(SizeSpec::FILL_W)
            .padding(Spacing::new(margin, margin, margin, 0.0))
            .show(ui, |ui| {
                let controls = Panel::hstack()
                    .id(band_wid(graph).with("controls"))
                    .align(Align::TOP_LEFT)
                    .child_align(VAlign::Center);
                pill::dress(controls, theme)
                    .max_size((controls_w_max, f32::INFINITY))
                    .clip(ClipMode::Rounded)
                    .show(ui, |ui| {
                        run_controls(ui, stage);
                        Panel::hstack()
                            .id(band_wid(graph).with("path"))
                            .padding(Spacing::xy(theme.space.xs, 0.0))
                            .gap(theme.space.sm)
                            .child_align(VAlign::Center)
                            .show(ui, |ui| match graph {
                                GraphId::Main => {
                                    let bold = TextStyle {
                                        weight: FontWeight::BOLD,
                                        ..support::body_text(ui, theme)
                                    };
                                    Text::new(MAIN).style(&bold).show(ui);
                                }
                                GraphId::Compound(compound) => {
                                    self.compound_path(ui, stage, compound);
                                }
                            });
                    });
                Panel::hstack()
                    .id(right_wid(graph))
                    .align(Align::TOP_RIGHT)
                    .gap(margin)
                    .show(ui, |ui| {
                        if let GraphId::Compound(compound) = graph {
                            let actions = Panel::hstack()
                                .id(band_wid(graph).with("actions"))
                                .child_align(VAlign::Center);
                            pill::dress(actions, theme).show(ui, |ui| {
                                self.compound_actions(ui, stage, compound);
                            });
                        }
                        let zoom = Panel::hstack().id(zoom_pill_wid(graph));
                        pill::dress(zoom, theme)
                            .show(ui, |ui| zoom_control(ui, stage, &mut self.zoom_scrub));
                    });
            });
    }
}

/// What the main graph is called, on its tab and in each path.
pub(crate) const MAIN: &str = "Main";

/// The run chip, the event loop's and Run on Change's: Run while idle, Cancel Run while a run is
/// in flight.
fn run_controls(ui: &mut Ui, stage: &mut Stage<'_, GraphCtx<'_>>) {
    let (graph_ctx, out) = (stage.ctx, &mut *stage.out);
    let (graph, theme) = (graph_ctx.graph(), graph_ctx.theme());
    let activity = graph_ctx.run_state().activity();
    let running = activity.is_executing();
    let run = if running {
        Command::CancelRun
    } else {
        Command::Run
    };
    // Run alone idles with a status-colored glyph; the event loop's sits muted beside it.
    if Chip::command(run_chip_wid(graph), run)
        .selected(running)
        .idle_glyph(theme.status.success)
        .ink(theme.status.busy)
        .show_icon(ui, theme, Icon::Run)
    {
        run.raise(ui, graph_ctx, out);
    }
    // The runs, Run on Change among them, in the chip's context menu too.
    Menu::context(&ui.response_for(run_chip_wid(graph))).show(ui, |ui| {
        menu::record(ui, RUN_MENU, graph_ctx, out, Unavailable::Greyed);
    });
    if Chip::command(band_wid(graph).with("events"), Command::ToggleEvents)
        .selected(activity.event_loop_active())
        .ink(theme.status.busy)
        .show_icon(ui, theme, Icon::RunEvents)
    {
        Command::ToggleEvents.raise(ui, graph_ctx, out);
    }
    // A preference, not a run in flight: the accent's on state, as a toggle's.
    if Chip::command(
        band_wid(graph).with("run_on_change"),
        Command::ToggleRunOnChange,
    )
    .selected(graph_ctx.run_mode() == RunMode::OnChange)
    .show_icon(ui, theme, Icon::RunOnChange)
    {
        Command::ToggleRunOnChange.raise(ui, graph_ctx, out);
    }
}

/// The zoom as a control: the camera's zoom in the monospace face, one width whatever its
/// figures, which opens the camera's commands below itself on a click and zooms about the pane's
/// center while dragged ([`pan_zoom::scrub_target`]), from the camera `scrub` holds since the
/// drag latched. A drag ends without the click, so it opens no menu.
fn zoom_control(ui: &mut Ui, stage: &mut Stage<'_, GraphCtx<'_>>, scrub: &mut Option<Viewport>) {
    let (graph_ctx, out) = (stage.ctx, &mut *stage.out);
    let (graph, theme) = (graph_ctx.graph(), graph_ctx.theme());
    let wid = zoom_control_wid(graph);
    let style = TextStyle {
        family: FontFamily::MONO,
        ..ui.theme().text
    };
    let label_w = ui
        .probe_text(TextRun {
            text: WIDEST_ZOOM,
            font: style.font(),
            wrap: TextWrap::SingleLine,
            align: Align::LEFT,
            max_width: None,
        })
        .size()
        .w;
    let viewport = graph_ctx.view().viewport;
    let zoom = Percent::new(viewport.zoom);
    let control = Button::new()
        .id(wid)
        .label(fmt!(ui, "{zoom}"))
        .style(&theme.mono_button)
        // Measured to fit its widest label exactly, which a truncating label can cut at the last
        // glyph over a rounding.
        .text_wrap(TextWrap::SingleLine)
        // Its own padding, which the width counts, in place of the theme's.
        .padding(Spacing::xy(theme.space.sm, 0.0))
        // As tall as a chip, so its pill lines up with the controls' pill.
        .size((
            Sizing::fixed(label_w + 2.0 * theme.space.sm),
            Sizing::fixed(theme.icon.toolbar.side()),
        ))
        .sense(Sense::CLICK | Sense::DRAG)
        .focusable(false)
        .show(ui);
    support::tooltip_after(ui, &control, Some("Zoom — drag to zoom"));
    match control.left.drag.delta() {
        Some(travel) => {
            let anchor = *scrub.get_or_insert(viewport);
            if let Some(pane) = ui.response_for(canvas::outer_wid(graph)).layout_rect {
                let to =
                    pan_zoom::scrub_target(anchor, travel, Vec2::new(pane.size.w, pane.size.h));
                // The camera gesture's test: the zoom-about-center algebra leaves last-bit noise.
                if !(to.pan.approximately_eq(viewport.pan)
                    && to.zoom.approximately_eq(viewport.zoom))
                {
                    out.push_graph(graph, GraphIntent::SetViewport { to });
                }
            }
        }
        None => *scrub = None,
    }
    if control.hovered() || scrub.is_some() {
        ui.request_cursor(CursorIcon::Move);
    }
    Menu::below(&control).show(ui, |ui| {
        menu::record(ui, ZOOM_MENU, graph_ctx, out, Unavailable::Greyed);
    });
}

/// The widest label the zoom shows: `Percent` writes the canvas's 10 % to 500 % in three or four
/// glyphs, and the percent sign is measured with them, as it need not advance as a figure does.
const WIDEST_ZOOM: &str = "500%";

/// The band over `graph`'s pane.
fn band_wid(graph: GraphId) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.pane_header", graph))
}

/// The right side of the band over `graph`'s pane: a compound's actions and the zoom.
fn right_wid(graph: GraphId) -> WidgetId {
    band_wid(graph).with("right")
}

/// The zoom control's pill in the band over `graph`'s pane.
fn zoom_pill_wid(graph: GraphId) -> WidgetId {
    band_wid(graph).with("zoom_pill")
}

/// The run chip in the band over `graph`'s pane.
pub(crate) fn run_chip_wid(graph: GraphId) -> WidgetId {
    band_wid(graph).with("run")
}

/// The zoom control in the band over `graph`'s pane.
pub(crate) fn zoom_control_wid(graph: GraphId) -> WidgetId {
    band_wid(graph).with("zoom")
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
    use glam::Vec2;
    use palantir::ColorScheme;
    use scenarium::{Compound, CompoundId, GraphId, InstancePath, Interface, Node, NodeId};

    use crate::core::document::Document;
    use crate::core::document::internals::{DocumentFixture, insert_compound};
    use crate::gui::pane::graph_ui::pane_header::{Crumb, CrumbTarget, for_each_crumb};
    use crate::gui::theme::Theme;

    /// The band floats over the main graph's canvas, which fills the pane from its top, reads
    /// Main, and its zoom control lists the camera's commands, each raising its framing of the
    /// pane it sits on.
    #[test]
    fn the_band_names_the_pane_and_its_zoom_lists_the_camera() {
        use crate::gui::app::session::internals::SessionHarness;
        use crate::gui::command::Command;
        use crate::gui::frame::app_command::AppCommand;
        use crate::gui::frame::framing::Framing;
        use crate::gui::frame::pane_ask::PaneAsk;
        use crate::gui::pane::graph_ui::canvas;
        use crate::gui::pane::graph_ui::pane_header::{MAIN, zoom_control_wid};

        let mut h = SessionHarness::new(DocumentFixture::probes(1));
        h.prime(2);
        let band = h.ui.rect(super::band_wid(GraphId::Main)).expect("the band");
        let canvas =
            h.ui.rect(canvas::outer_wid(GraphId::Main))
                .expect("the canvas");
        assert_eq!(canvas.min, band.min, "the band over the canvas's top");
        assert!(
            h.ui.widgets(Some(MAIN))
                .iter()
                .any(|widget| widget.text == MAIN)
        );
        let zoom = h.ui.rect(zoom_control_wid(GraphId::Main)).unwrap();
        assert!(
            h.ui.widgets(Some("100%"))
                .iter()
                .any(|widget| widget.text == "100%")
        );

        h.ui.click(zoom.center());
        h.frame();
        h.prime(2);
        let label = Command::ZoomToFit.spec().label;
        let row =
            h.ui.widgets(Some(label))
                .into_iter()
                .find(|widget| widget.text.starts_with(label))
                .expect("the zoom control lists Zoom to Fit");
        h.ui.click(row.id);
        let commands = h.frame();
        assert!(
            matches!(
                commands[..],
                [AppCommand::Pane {
                    graph: GraphId::Main,
                    ask: PaneAsk::Frame(Framing::Fit)
                }]
            ),
            "{commands:?}"
        );
    }

    /// A drag on the zoom control zooms the camera, 200 px right from 100 % to 200 %, then 400 px
    /// down from there to 50 %, and opens no menu; its width holds while its figures go from four
    /// glyphs to three.
    #[test]
    fn a_drag_on_the_zoom_control_zooms_and_holds_its_width() {
        use palantir::PointerButton;

        use crate::gui::app::session::internals::SessionHarness;
        use crate::gui::command::Command;
        use crate::gui::pane::graph_ui::pane_header::zoom_control_wid;

        let mut h = SessionHarness::new(DocumentFixture::probes(1));
        h.prime(2);
        let control = h.ui.rect(zoom_control_wid(GraphId::Main)).unwrap();
        let drag = |h: &mut SessionHarness, by: Vec2| {
            let at = control.center();
            h.ui.press(at, PointerButton::Left);
            h.frame();
            h.ui.drag_to(at + by);
            h.prime(2);
            h.ui.release(PointerButton::Left);
            h.prime(2);
            h.session.open.document().main_view.viewport.zoom
        };
        assert_eq!(drag(&mut h, Vec2::new(200.0, 0.0)), 2.0);
        assert_eq!(drag(&mut h, Vec2::new(0.0, 400.0)), 0.5);
        let label = Command::ZoomToFit.spec().label;
        assert!(
            !h.ui
                .widgets(Some(label))
                .iter()
                .any(|widget| widget.text.starts_with(label)),
            "a drag opens no menu"
        );
        assert!(
            h.ui.widgets(Some("50%"))
                .iter()
                .any(|widget| widget.text == "50%")
        );
        let after = h.ui.rect(zoom_control_wid(GraphId::Main)).unwrap();
        assert_eq!(after.size, control.size);
    }

    /// A compound's band in a 500 px pane, its path, count of uses and actions wider than the
    /// pane, cuts the pane's name: the controls' pill narrows from its width in a 1400 px pane and
    /// ends at least a margin clear of the right side, whose actions keep their places, Lock as in
    /// the wide pane, and which stands whole a margin inside the pane.
    #[test]
    fn a_compound_band_too_wide_for_its_pane_cuts_the_name() {
        use glam::UVec2;

        use crate::gui::app::session::internals::SessionHarness;

        let mut fixture = DocumentFixture::default();
        let compound = CompoundId::from_u128(1);
        insert_compound(
            &mut fixture.document,
            compound,
            Compound::new(
                "A compound named past any narrow pane",
                Interface::default(),
            ),
        );
        let instance = fixture.add_instance(compound);
        fixture.add_instance(compound);
        fixture
            .document
            .enter(compound, Some(InstancePath::new(&[instance])));
        let mut h = SessionHarness::new(fixture);
        let graph = GraphId::Compound(compound);
        let rects = |h: &SessionHarness| {
            let rect = |id| h.ui.rect(id).expect("recorded");
            [
                rect(super::band_wid(graph)),
                rect(super::band_wid(graph).with("controls")),
                rect(super::right_wid(graph)),
                rect(super::library_wid(compound, "lock")),
            ]
        };
        h.ui.resize(UVec2::new(1400, 800));
        h.prime(2);
        let [_, wide_controls, _, wide_lock] = rects(&h);
        h.ui.resize(UVec2::new(500, 800));
        h.prime(2);
        let [band, controls, right, lock] = rects(&h);
        let margin = Theme::new(ColorScheme::Dark).space.sm;
        assert_eq!(band.size.w, 500.0, "{band:?}");
        assert!(controls.size.w < wide_controls.size.w, "{controls:?}");
        assert_eq!(lock.size, wide_lock.size, "Lock reads whole");
        assert!(right.min.x <= lock.min.x, "{lock:?} in {right:?}");
        assert!(lock.max().x <= right.max().x, "{lock:?} in {right:?}");
        assert!(
            controls.max().x + margin <= right.min.x,
            "{controls:?} before {right:?}"
        );
        assert_eq!(
            right.max().x,
            band.max().x - margin,
            "{right:?} in {band:?}"
        );
    }

    /// The band's chips act: Run on Change, on at the start, asks for Manual; on a compound placed
    /// twice and shown through one instance, Lock locks it and shows its on state, a second press
    /// unlocks it, and Make Unique gives that instance a copy of its own, which the pane then
    /// shows, the compound left with its other use.
    #[test]
    fn the_bands_chips_toggle_the_run_mode_the_lock_and_make_unique() {
        use crate::core::io::preferences::run_mode::RunMode;
        use crate::gui::app::session::internals::SessionHarness;
        use crate::gui::frame::app_command::AppCommand;

        let mut fixture = DocumentFixture::default();
        let compound = CompoundId::from_u128(1);
        insert_compound(
            &mut fixture.document,
            compound,
            Compound::new("Adjust", Interface::default()),
        );
        let instance = fixture.add_instance(compound);
        fixture.add_instance(compound);
        fixture
            .document
            .enter(compound, Some(InstancePath::new(&[instance])));
        let mut h = SessionHarness::new(fixture);
        h.prime(2);
        let graph = GraphId::Compound(compound);
        let press = |h: &mut SessionHarness, id| {
            let at = h.ui.rect(id).expect("the chip").center();
            h.ui.click(at);
            let commands = h.frame();
            h.prime(2);
            commands
        };

        let commands = press(&mut h, super::band_wid(graph).with("run_on_change"));
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, AppCommand::SetRunMode(RunMode::Manual))),
            "{commands:?}"
        );

        let locked = |h: &SessionHarness| h.session.open.document().sides[&compound].locked;
        press(&mut h, super::library_wid(compound, "lock"));
        assert!(locked(&h), "locked");
        press(&mut h, super::library_wid(compound, "lock"));
        assert!(!locked(&h), "unlocked");

        press(&mut h, super::make_unique_wid(compound));
        let document = h.session.open.document();
        assert_eq!(document.graphs.uses(compound).count(), 1);
        let GraphId::Compound(copy) = document.active else {
            panic!("the pane shows a compound");
        };
        assert_ne!(copy, compound, "the pane shows the copy");
        assert_eq!(document.graphs.uses(copy).count(), 1);
    }

    /// A compound that changed here and in the library offers Update and Publish, and no Follow:
    /// a first press on Publish arms it and shows the check and the cross, the cross disarms it
    /// with no write, and the check asks for the publish once. Update then puts the library's
    /// definition in place, so the two stand in sync: Update and Publish go, and Follow comes, whose
    /// press makes the copy follow the library and shows its on state.
    #[test]
    fn the_library_chips_update_publish_once_asked_and_follow() {
        use common::TempDir;
        use scenarium::Library;

        use crate::core::io::document;
        use crate::core::io::document::saved_paths::SavedPaths;
        use crate::core::shared_library::scan;
        use crate::gui::app::session::internals::SessionHarness;
        use crate::gui::frame::app_command::AppCommand;

        let dir = TempDir::new("darkroom-band-library");
        let compound = CompoundId::from_u128(1);
        let mut library_file = DocumentFixture::default();
        insert_compound(
            &mut library_file.document,
            compound,
            Compound::new("Adjust", Interface::default()),
        );
        library_file
            .document
            .sides
            .get_mut(&compound)
            .unwrap()
            .shared = true;
        document::save(
            &library_file.document,
            &Library::default(),
            &dir.join("library.darkroom"),
            SavedPaths::Relative,
        )
        .unwrap();

        let mut fixture = DocumentFixture::default();
        insert_compound(
            &mut fixture.document,
            compound,
            Compound::new("Adjust, changed here", Interface::default()),
        );
        let instance = fixture.add_instance(compound);
        fixture
            .document
            .enter(compound, Some(InstancePath::new(&[instance])));
        let mut h = SessionHarness::new(fixture);
        h.shared = scan(dir.path());
        h.prime(2);
        let chip = |h: &SessionHarness, part| h.ui.rect(super::library_wid(compound, part));
        let press = |h: &mut SessionHarness, part| {
            let at = chip(h, part).expect("the chip shows").center();
            h.ui.click(at);
            let commands = h.frame();
            h.prime(2);
            commands
        };
        let published = |commands: &[AppCommand]| {
            commands
                .iter()
                .filter(|command| matches!(command, AppCommand::Publish { .. }))
                .count()
        };
        assert!(chip(&h, "update").is_some());
        assert!(chip(&h, "follow").is_none(), "no Follow over local changes");
        assert!(chip(&h, "publish_yes").is_none(), "not armed");

        assert_eq!(published(&press(&mut h, "publish")), 0);
        assert!(chip(&h, "publish_yes").is_some(), "armed");
        assert_eq!(published(&press(&mut h, "publish_no")), 0);
        assert!(chip(&h, "publish_yes").is_none(), "disarmed");
        press(&mut h, "publish");
        assert_eq!(published(&press(&mut h, "publish_yes")), 1);
        assert!(chip(&h, "publish_yes").is_none(), "answered");

        press(&mut h, "update");
        let document = h.session.open.document();
        assert_eq!(
            document.graphs.compound(compound).unwrap().decl.name,
            "Adjust"
        );
        assert!(chip(&h, "update").is_none(), "in sync");
        assert!(chip(&h, "publish").is_none(), "in sync");
        press(&mut h, "follow");
        assert!(h.session.open.document().sides[&compound].follows);
    }

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
