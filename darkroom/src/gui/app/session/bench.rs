//! The editor frame: `bench.darkroom`, an astro pipeline saved from the editor, through the
//! editor's real widgets, a frame of it as `App` runs one, on each backend.
//!
//! The document holds its own workspace: the graph, an image viewer and the inspector docked
//! around it, at the camera it was saved with. Every preview node shows a 24 MP float image,
//! planar as a stacked frame lumos publishes. Its inputs are absent, as no case runs the graph.
//! The viewer shrinks that image with one tap, not the editor's area average, which would hide the
//! rest of a `pan` or `zoom` frame: 0.6 ms on an Apple laptop.
//!
//! - **`cpu`**: record through encode and compose on palantir's deviceless frontend, as the
//!   palantir frame's `cpu` backend does.
//! - **`gpu`**: palantir's `OffscreenHost` frame against an offscreen texture, waited for, on
//!   the desktop's present strategy.
//!
//! Scenarios, on both backends:
//!
//! - **`cached`**: nothing changes, so the frame plans no paint.
//! - **`pan`**: the camera moves, as a drag on the canvas moves it.
//! - **`zoom`**: the camera zooms between two scales, so every body re-lays out its text.
//! - **`drag`**: one node moves, and the wires on it reroute.
//! - **`preview`**: the preview node the viewer shows publishes a new 24 MP image, as a run
//!   publishes one.
//! - **`click`**, CPU alone: a press and a release in turn on bare canvas, which the input stage
//!   handles, so each frame records once.
//! - **`typing`**, CPU alone: a key and a Backspace in turn in a text const, which a record reads,
//!   so each frame records twice.
//!
//! A scenario changes the editor as production does: the camera and the drag are one held
//! gesture of edit intents through `Session::apply_edit`, whose relayout the frame takes.

use common::{Buffer2, USize2};
use imaginarium::PlanarImage;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use criterion::Criterion;
use glam::Vec2;
use lens::MlModelPaths;
use palantir::bench::backends::{Backend, Backends};
use palantir::bench::case_group::CaseGroup;
use palantir::bench::kind::{Kind, Scale};
use palantir::bench::scene::{BENCH_DPR, BENCH_SURFACE};
use palantir::internals::Timing;
use palantir::{
    App, ColorScheme, Depth, Harness, InputOutcome, Key, PointerButton, UpdateCtx, WindowToken,
};
use palantir::{FramePaint, ImageDownsample, OffscreenHost, Ui};
use scenarium::{DynamicValue, NodeId};

use crate::core::document::Document;
use crate::core::document::item_id::ItemId;
use crate::core::document::viewport::Viewport;
use crate::core::edit::gesture_id::GestureId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::drag_start::DragStart;
use crate::core::io::preferences::Preferences;
use crate::core::open_document::{Loaded, OpenDocument};
use crate::core::runtime_library::RuntimeLibrary;
use crate::core::wake;
use crate::gui::app::session::Session;
use crate::gui::frame::app_command::{AppCommand, ViewCommand};
use crate::gui::frame::app_ctx::AppCtx;
use crate::gui::frame::frame_writes::FrameWrites;
use crate::gui::frame::requests::Requests;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::state::preview_store::preview_converter::{self, Card};
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

/// The document, from the crate's root.
const DOCUMENT: &str = "src/gui/app/session/bench.darkroom";
/// The blend the `drag` scenario moves: on screen at the document's camera, with four wires on it.
const DRAGGED: NodeId = NodeId::literal("fabb092d-f756-441d-b6f5-1d1e629beda1");
/// The preview node the viewer's active tab shows, which the `preview` scenario publishes to.
const PUBLISHED: NodeId = NodeId::literal("e156726f-3c5b-4d41-bbeb-380773608e40");
/// How far the `pan` scenario moves the camera each frame, and the frames before it starts over.
const PAN_STEP: Vec2 = Vec2::new(1.5, 0.7);
const PAN_FRAMES: u32 = 256;
/// The zoom the `zoom` scenario alternates with, as a share of the document's.
const ZOOM_IN: f32 = 1.1;
/// The most a case waits for the viewer's full image, which the converter's thread makes.
const FULL_IMAGE_WAIT: Duration = Duration::from_secs(60);
/// Where the `click` scenario presses, in the canvas: bare, past the toolbar and above the nodes.
const BARE_CANVAS: Vec2 = Vec2::new(80.0, 20.0);
/// The value of the text const the `typing` scenario edits.
const TEXT_CONST: &str = "frameNo_";
/// Frames each scenario runs before its paint is checked.
const SETTLE_FRAMES: u32 = 4;

/// What changes between two frames of a scenario.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    Cached,
    Drag,
    Pan,
    Preview,
    Zoom,
    Click,
    Typing,
}

impl Scenario {
    const ALL: [Self; 7] = [
        Self::Cached,
        Self::Drag,
        Self::Pan,
        Self::Preview,
        Self::Zoom,
        Self::Click,
        Self::Typing,
    ];

    /// Plays input, which a [`Harness`] frame takes, so the CPU backend alone.
    const fn plays_input(self) -> bool {
        matches!(self, Self::Click | Self::Typing)
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Cached => "cached",
            Self::Drag => "drag",
            Self::Pan => "pan",
            Self::Preview => "preview",
            Self::Zoom => "zoom",
            Self::Click => "click",
            Self::Typing => "typing",
        }
    }
}

/// The editor's state as `App` holds it, over the document.
#[derive(Debug)]
struct Editor {
    session: Session,
    library: RuntimeLibrary,
    theme: Theme,
    run_state: RunState,
    preferences: Preferences,
    requests: Requests,
    /// The document's preview nodes.
    previews: Vec<NodeId>,
    /// The camera the document was saved with, which the `pan` and `zoom` scenarios move from.
    camera: Viewport,
    /// What the `drag` scenario moves, from where it starts.
    dragged: Rc<[DragStart]>,
    /// Whether a viewer's ask for a full image reaches the converter, as it does in production.
    /// Off once the viewer holds one: the `preview` scenario publishes faster than a conversion
    /// runs, so it times what the viewer shows until its next one lands.
    converts_full: bool,
    /// The two images the `preview` scenario lands in turn, each converted once
    /// up front: in production the converter's thread does that, off every
    /// frame, so a frame here times the landing alone.
    converted: [Card; 2],
}

impl Editor {
    /// The document over the shipped library, every preview publishing an image.
    fn new() -> Self {
        let runtime = RuntimeLibrary::new(&MlModelPaths::default());
        let open = open_document(&runtime);
        let document = open.document();
        let previews = preview_nodes(document);
        let camera = document.main_view.viewport;
        let dragged = Rc::new([DragStart {
            item: ItemId::Node(DRAGGED),
            pos: document.main_view.node_placements[&DRAGGED].pos,
        }]);

        Self {
            session: Session::new(open),
            library: runtime,
            theme: Theme::default(),
            run_state: RunState::new(wake::internals::noop()),
            preferences: Preferences::default(),
            requests: Requests::default(),
            previews,
            camera,
            dragged,
            converts_full: true,
            converted: [0.2, 0.4]
                .map(|level| preview_converter::internals::convert_here(stacked_frame(level))),
        }
    }

    /// Land an image on every preview node, as a finished run does once its
    /// conversions finish.
    fn publish_previews(&mut self, ui: &mut Ui) {
        for (idx, node) in self.previews.iter().enumerate() {
            let card = self.converted[idx % 2].clone();
            self.run_state
                .previews_mut()
                .land_converted(&ui.update_ctx(), *node, card);
        }
    }

    /// Set the editor to frame `n` of `scenario`, through the path production takes.
    fn advance(&mut self, scenario: Scenario, ui: &mut Ui, n: u32) {
        // One held drag for the whole run, so its frames fold into one undo entry.
        let gesture = GestureId::default().next();
        self.run_state.previews_mut().land(&ui.update_ctx());
        let intent = match scenario {
            Scenario::Cached | Scenario::Click | Scenario::Typing => return,
            // Its publishes would replace the image the viewer is waiting for.
            Scenario::Preview if self.converts_full => return,
            Scenario::Preview => {
                let card = self.converted[(n % 2) as usize].clone();
                self.run_state
                    .previews_mut()
                    .land_converted(&ui.update_ctx(), PUBLISHED, card);
                return;
            }
            Scenario::Pan => {
                let mut to = self.camera;
                to.pan += PAN_STEP * (n % PAN_FRAMES) as f32;
                GraphIntent::SetViewport { to }
            }
            Scenario::Zoom => {
                let mut to = self.camera;
                if n.is_multiple_of(2) {
                    to.zoom *= ZOOM_IN;
                }
                GraphIntent::SetViewport { to }
            }
            Scenario::Drag => GraphIntent::MoveSelection {
                gesture,
                members: Rc::clone(&self.dragged),
                offset: Vec2::new(if n.is_multiple_of(2) { 1.0 } else { 0.0 }, 0.0),
            },
        };
        self.session
            .apply_edit(intent, self.library.cur(), &mut self.requests);
    }

    fn run_commands(&mut self) {
        while let Some(command) = self.requests.pop_app() {
            let AppCommand::View(ViewCommand::ConvertFull { node_id, max_dim }) = command else {
                panic!("a frame with no input raises only a viewer's ask: {command:?}");
            };
            if self.converts_full {
                self.run_state.previews_mut().convert_full(node_id, max_dim);
            }
        }
    }
}

/// The sweeps, the input stage and the record passes as `App` runs them.
impl App for Editor {
    fn update(&mut self, _: WindowToken, mut update_ctx: UpdateCtx<'_>) -> InputOutcome {
        self.run_state
            .previews_mut()
            .reconcile(self.session.open.document());
        self.session.reconcile_caches();
        let ctx = AppCtx::new(&self.theme, self.library.cur(), &self.run_state);
        let outcome = self.session.input(&mut update_ctx, ctx, &mut self.requests);
        self.run_commands();
        outcome
    }

    fn record(&mut self, _: WindowToken, ui: &mut Ui) {
        let ctx = AppCtx::new(&self.theme, self.library.cur(), &self.run_state);
        self.session.record(
            ui,
            ctx,
            FrameWrites {
                preferences: &mut self.preferences,
                requests: &mut self.requests,
            },
        );
        self.run_commands();
        self.requests.spend_relayout(ui);
    }
}

/// The document as the editor opens it. A repair would detach a node and measure another document,
/// so it stops the case.
fn open_document(library: &RuntimeLibrary) -> OpenDocument {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(DOCUMENT);
    let Loaded { open, report } =
        OpenDocument::load(path, library.cur()).expect("the bench document loads");
    assert!(report.is_empty(), "the bench document is stale: {report:?}");
    open
}

fn preview_nodes(document: &Document) -> Vec<NodeId> {
    document
        .graph
        .iter()
        .map(|node| node.node_id)
        .filter(|&node| document.holds_preview_node(node))
        .collect()
}

/// A 6000 × 4000 frame of float RGB at `level`, one plane per channel: a stacked frame as lumos
/// publishes one.
fn stacked_frame(level: f32) -> DynamicValue {
    let size = USize2::new(6000, 4000);
    let planes = std::array::from_fn(|_| Buffer2::new_filled(size, level));
    DynamicValue::from_custom(lens::Image::from(PlanarImage::new_rgb(planes)))
}

/// The input of frame `n` of `scenario`. Each press leaves the double-click span behind, so each is
/// its own click. The focus goes to the text const on the first frame it shows.
fn play_input(scenario: Scenario, frontend: &mut Harness, n: u32) {
    match scenario {
        // The first frames lay the canvas out, so a press has a rect to land in.
        Scenario::Click if n < 2 => {}
        Scenario::Click => {
            if n.is_multiple_of(2) {
                frontend.advance_past_double_click();
                let bare = frontend.point_in(canvas::outer_wid(), BARE_CANVAS);
                frontend.press(bare, PointerButton::Left);
            } else {
                frontend.release(PointerButton::Left);
            }
        }
        Scenario::Typing => {
            if frontend.ui().focus().is_none() {
                let field = frontend.widgets(Some(TEXT_CONST)).first().map(|w| w.id);
                if let Some(field) = field {
                    frontend.ui_mut().set_focus(field);
                }
            } else if n.is_multiple_of(2) {
                frontend.key(Key::Char('x'));
            } else {
                frontend.key(Key::Backspace);
            }
        }
        _ => {}
    }
}

/// One editor-frame case: the document in `scenario`, painted on `backend`.
#[derive(Debug, Clone, Copy)]
struct FrameCase {
    scenario: Scenario,
    backend: Backend,
}

impl FrameCase {
    /// Measure the case: build the frame with `build` in the case, run it
    /// through [`SETTLE_FRAMES`], check its paint, then time frames.
    fn measure<F>(self, group: &mut CaseGroup<'_>, build: impl Fn() -> F)
    where
        F: FnMut(&mut Editor, u32) -> FramePaint,
    {
        let Self { scenario, backend } = self;
        group.bench_function(backend.case(scenario.label()), |b| {
            let mut frame = build();
            let mut editor = Editor::new();
            let mut n = 0u32;
            let mut step = |editor: &mut Editor| {
                let paint = frame(editor, n);
                n = n.wrapping_add(1);
                paint
            };
            let asked = Instant::now();
            while !editor.run_state.previews().full_landed(PUBLISHED) {
                assert!(
                    asked.elapsed() < FULL_IMAGE_WAIT,
                    "the viewer's image lands"
                );
                step(&mut editor);
            }
            editor.converts_full = false;
            editor
                .session
                .main_window
                .image_viewers
                .get_mut(&PUBLISHED)
                .expect("the published node's tab is the one the viewer shows")
                .set_downsample(ImageDownsample::Single);
            for _ in 0..SETTLE_FRAMES {
                step(&mut editor);
            }
            let paint = step(&mut editor);
            if scenario == Scenario::Cached {
                assert_eq!(paint, FramePaint::Skip, "a still editor paints nothing");
            } else if scenario != Scenario::Click {
                assert_ne!(
                    paint,
                    FramePaint::Skip,
                    "{} reaches paint",
                    scenario.label()
                );
            }
            b.iter(|| step(&mut editor));
        });
    }
}

/// `integration/editor_frame/<backend>/<scenario>`: one whole editor frame of the document.
pub(crate) fn integration(c: &mut Criterion, backends: &Backends) {
    let mut group = Kind::Integration.group(c, "editor_frame", Scale::Frame);
    for backend in backends.all() {
        for scenario in Scenario::ALL {
            if scenario.plays_input() && backend != Backend::Cpu {
                continue;
            }
            let case = FrameCase { scenario, backend };
            match backend {
                Backend::Cpu => case.measure(&mut group, || {
                    let mut frontend = Harness::builder(BENCH_SURFACE)
                        .system_scale(BENCH_DPR)
                        .depth(Depth::Compose)
                        .build();
                    frontend
                        .ui_mut()
                        .set_theme(ColorScheme::Dark, Theme::default().palantir);
                    let mut published = false;
                    move |editor: &mut Editor, n: u32| {
                        let ui = frontend.ui_mut();
                        if !published {
                            editor.publish_previews(ui);
                            published = true;
                        }
                        editor.advance(scenario, ui, n);
                        play_input(scenario, &mut frontend, n);
                        frontend.frame_app(editor).paint()
                    }
                }),
                Backend::Gpu => case.measure(&mut group, || {
                    let gpu = backends.device(Timing::PassOnly);
                    let target = gpu.target("darkroom.editor_frame", BENCH_SURFACE);
                    let mut host = OffscreenHost::builder(gpu.handles())
                        .collect_gpu_stats(false)
                        .swapchain_present()
                        .build();
                    host.ui_mut()
                        .set_theme(ColorScheme::Dark, Theme::default().palantir);
                    let mut published = false;
                    move |editor: &mut Editor, n: u32| {
                        if !published {
                            editor.publish_previews(host.ui_mut());
                            published = true;
                        }
                        editor.advance(scenario, host.ui_mut(), n);
                        let paint = host
                            .set_system_scale(BENCH_DPR)
                            .frame(target.as_target(), editor)
                            .paint();
                        gpu.wait();
                        paint
                    }
                }),
            }
        }
    }
    group.finish();
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use lens::MlModelPaths;

    use crate::core::runtime_library::RuntimeLibrary;
    use crate::gui::app::session::bench::{
        DOCUMENT, DRAGGED, PUBLISHED, open_document, preview_nodes,
    };

    /// The document opens with nothing to repair, and still holds what the cases move: 4 preview
    /// nodes, and the dragged node on the canvas. Its 65 paths, 60 frames, 2 models and 3 outputs,
    /// stay inside its folder, so it names nothing of the machine it was saved on.
    #[test]
    fn the_document_opens_clean_and_holds_what_the_cases_move() {
        let open = open_document(&RuntimeLibrary::new(&MlModelPaths::default()));
        let document = open.document();

        let previews = preview_nodes(document);
        assert_eq!(previews.len(), 4);
        assert!(previews.contains(&PUBLISHED));
        assert!(document.main_view.node_placements.contains_key(&DRAGGED));

        let mut paths = Vec::new();
        document.for_each_path(|_, path| paths.push(PathBuf::from(path)));
        assert_eq!(paths.len(), 65);
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join(DOCUMENT);
        let folder = file.parent().unwrap();
        for path in &paths {
            assert!(path.starts_with(folder), "{}", path.display());
        }
    }
}
