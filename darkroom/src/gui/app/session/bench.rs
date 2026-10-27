//! The editor frame: a made graph through the editor's real widgets, a frame of it as `App`
//! runs one, on each backend.
//!
//! The graph is [`CHAINS`] chains of the shipped image nodes, each `Load Image` → `Brightness /
//! Contrast` → `Transform` → `Blend` → preview, the blend's second layer wired from the chain
//! above: 200 nodes and 200 wires, in rows on the canvas at half zoom, so the window shows a
//! screenful and culls the rest. The rows are spaced from the nodes' own measured sizes, so no
//! node covers another. Every preview shows a 24 MP float image, planar as a stacked frame lumos
//! publishes.
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
//! - **`preview`**: one preview node publishes a new 24 MP image, as a run publishes one.
//!
//! A scenario changes the editor as production does: the camera and the drag are one held
//! gesture of edit intents through `OpenDocument::apply_edit`, whose relayout the frame takes.

use common::{Buffer2, Size2us};
use imaginarium::PlanarImage;
use std::sync::Arc;

use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion};
use glam::Vec2;
use lens::MlModelPaths;
use palantir::bench::fixture::{Backend, Backends, Kind, Scale};
use palantir::internals::frame_fixture::{BENCH_DPR, BENCH_SURFACE};
use palantir::internals::record_app::RecordApp;
use palantir::internals::ui_harness::UiHarness;
use palantir::internals::ui_harness::frontend_harness::FrontendHarness;
use palantir::internals::{BenchGpu, Timing};
use palantir::{FramePaint, OffscreenHost, Ui};
use scenarium::{Binding, DynamicValue, Func, Graph, InputPort, Library, NodeId};

use crate::core::document::Document;
use crate::core::document::item_id::ItemId;
use crate::core::edit::gesture_id::GestureId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::drag_start::DragStart;
use crate::core::edit::relayout::Relayout;
use crate::core::io::preferences::Preferences;
use crate::core::open_document::OpenDocument;
use crate::core::preview_sink;
use crate::core::runtime_library::RuntimeLibrary;
use crate::core::wake;
use crate::gui::app::session::Session;
use crate::gui::frame::app_ctx::AppCtx;
use crate::gui::frame::frame_writes::FrameWrites;
use crate::gui::frame::requests::Requests;
use crate::gui::pane::graph_ui::node_ui::wid;
use crate::gui::state::preview_store::preview_converter::{self, Card};
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

/// The chains of the made graph, five nodes and five wires each.
const CHAINS: usize = 40;
/// Canvas-world space between two nodes of a chain, and between two chains: room for the wires.
const GAP: f32 = 60.0;
/// The chain whose blend the `drag` scenario moves: on screen at the camera's zoom.
const DRAGGED_CHAIN: usize = 1;
/// A chain's transform, its third node, whose output the chain below blends in.
const TRANSFORM: usize = 2;
/// A chain's blend, its fourth node.
const BLEND: usize = 3;
/// A chain's preview, its last node.
const PREVIEW: usize = 4;
/// The camera's zoom, and the one the `zoom` scenario alternates with.
const ZOOM: f32 = 0.5;
const ZOOM_IN: f32 = 0.55;
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
}

impl Scenario {
    const ALL: [Self; 5] = [
        Self::Cached,
        Self::Drag,
        Self::Pan,
        Self::Preview,
        Self::Zoom,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Cached => "cached",
            Self::Drag => "drag",
            Self::Pan => "pan",
            Self::Preview => "preview",
            Self::Zoom => "zoom",
        }
    }
}

/// The editor's state as `App` holds it, over the made graph.
#[derive(Debug)]
struct Editor {
    session: Session,
    library: Arc<Library>,
    theme: Theme,
    run_state: RunState,
    preferences: Preferences,
    requests: Requests,
    /// Each chain's nodes, from its `Load Image` to its preview.
    chains: Vec<[NodeId; 5]>,
    /// Whether the chains sit where [`Self::lay_out`] places them, rather than all at the origin.
    laid_out: bool,
    /// What the `drag` scenario moves, from where it starts: the second chain's blend, on screen
    /// at the camera's zoom, which three wires reach.
    dragged: Arc<[DragStart]>,
    /// The two images the `preview` scenario lands in turn, each converted once
    /// up front: in production the converter's thread does that, off every
    /// frame, so a frame here times the landing alone.
    converted: [Card; 2],
}

impl Editor {
    /// The made graph over the shipped library, every preview publishing an image.
    fn new() -> Self {
        let runtime = RuntimeLibrary::new(&MlModelPaths::default());
        let library = Arc::clone(runtime.cur());
        let func = |name: &str| -> Func {
            library
                .by_name(name)
                .unwrap_or_else(|| panic!("darkroom ships the {name:?} node"))
                .clone()
        };
        let load = func("Load Image");
        let adjust = func("Brightness / Contrast");
        let transform = func("Transform");
        let blend = func("Blend");
        let show = preview_sink::registered(&library)
            .expect("the runtime library holds the preview node")
            .clone();

        let mut graph = Graph::default();
        let mut chains = Vec::with_capacity(CHAINS);
        let mut transform_above = None;
        for _ in 0..CHAINS {
            let nodes = [&load, &adjust, &transform, &blend, &show].map(|f| graph.add_func_node(f));
            for pair in nodes.windows(2) {
                graph.set_input_binding(InputPort::new(pair[1], 0), Binding::wire(pair[0], 0));
            }
            let layer = transform_above.unwrap_or(nodes[TRANSFORM]);
            graph.set_input_binding(InputPort::new(nodes[BLEND], 1), Binding::wire(layer, 0));
            transform_above = Some(nodes[TRANSFORM]);
            chains.push(nodes);
        }

        // Every node starts at the origin, on screen, where the first frame measures it.
        let mut document = Document::from(graph);
        document.main_view.viewport.zoom = ZOOM;

        Self {
            session: Session::new(OpenDocument::over(document)),
            library,
            theme: Theme::default(),
            run_state: RunState::new(wake::internals::noop()),
            preferences: Preferences::default(),
            requests: Requests::default(),
            dragged: Arc::new([DragStart {
                item: ItemId::Node(chains[DRAGGED_CHAIN][BLEND]),
                pos: Vec2::ZERO,
            }]),
            chains,
            laid_out: false,
            converted: [0.2, 0.4]
                .map(|level| preview_converter::internals::convert_here(stacked_frame(level))),
        }
    }

    /// Land an image on every preview node, as a finished run does once its
    /// conversions finish.
    fn publish_previews(&mut self, ui: &Ui) {
        for (idx, chain) in self.chains.iter().enumerate() {
            let card = self.converted[idx % 2].clone();
            self.run_state
                .previews
                .land_converted(ui, chain[PREVIEW], card);
        }
    }

    /// Place the chains in rows from the size last frame measured for each node of the first:
    /// every chain holds the same nodes, so each column is as wide as its node there, and each
    /// row as tall as the tallest, with [`GAP`] between them. `false` while the first chain has
    /// not been measured yet.
    fn lay_out(&mut self, ui: &Ui) -> bool {
        let sizes = self.chains[0].map(|node| ui.response_for(wid::body(node)).layout_rect);
        let Some(sizes) = sizes.into_iter().collect::<Option<Vec<_>>>() else {
            return false;
        };
        let cols: Vec<f32> = sizes
            .iter()
            .scan(0.0, |x, rect| {
                let at = *x;
                *x += rect.size.w + GAP;
                Some(at)
            })
            .collect();
        let row = sizes.iter().map(|r| r.size.h).fold(0.0, f32::max) + GAP;
        let placements = &mut self.session.open.document.main_view.node_placements;
        for (chain, nodes) in self.chains.iter().enumerate() {
            for (col, node) in nodes.iter().enumerate() {
                placements
                    .get_mut(node)
                    .expect("every graph node is placed")
                    .pos = Vec2::new(cols[col], chain as f32 * row);
            }
        }
        self.dragged = Arc::new([DragStart {
            item: ItemId::Node(self.chains[DRAGGED_CHAIN][BLEND]),
            pos: Vec2::new(cols[BLEND], DRAGGED_CHAIN as f32 * row),
        }]);
        true
    }

    /// Set the editor to frame `n` of `scenario`, through the path production takes.
    fn advance(&mut self, scenario: Scenario, ui: &mut Ui, n: u32) {
        // One held gesture for the whole run, so its frames fold into one undo entry.
        let gesture = GestureId::default().next();
        let viewport = self.session.open.document.main_view.viewport;
        let intent = match scenario {
            Scenario::Cached => return,
            Scenario::Preview => {
                let card = self.converted[(n % 2) as usize].clone();
                self.run_state
                    .previews
                    .land_converted(ui, self.chains[0][PREVIEW], card);
                return;
            }
            Scenario::Pan => {
                let mut to = viewport;
                to.pan = Vec2::new((to.pan.x + 1.5) % 256.0, (to.pan.y + 0.7) % 256.0);
                GraphIntent::SetViewport {
                    to,
                    gesture: Some(gesture),
                }
            }
            Scenario::Zoom => {
                let mut to = viewport;
                to.zoom = if n.is_multiple_of(2) { ZOOM_IN } else { ZOOM };
                GraphIntent::SetViewport {
                    to,
                    gesture: Some(gesture),
                }
            }
            Scenario::Drag => GraphIntent::MoveSelection {
                gesture,
                members: Arc::clone(&self.dragged),
                offset: Vec2::new(if n.is_multiple_of(2) { 1.0 } else { 0.0 }, 0.0),
            },
        };
        if self.session.open.apply_edit(intent, &self.library) == Relayout::Needed {
            ui.request_relayout();
        }
    }

    /// One record pass as `App` runs a frame: the sweeps of `App::update`, then `App::record`.
    /// The pass after the first lays the chains out from what the first measured.
    fn record(&mut self, ui: &mut Ui) {
        if !self.laid_out && self.lay_out(ui) {
            self.laid_out = true;
            ui.request_relayout();
        }
        self.run_state
            .previews
            .reconcile(&self.session.open.document);
        self.session.reconcile_caches();
        let ctx = AppCtx::new(&self.theme, &self.library, &self.run_state);
        self.requests.clear();
        let relayout = self.session.frame(
            ui,
            ctx,
            FrameWrites {
                preferences: &mut self.preferences,
                requests: &mut self.requests,
            },
        );
        assert!(
            self.requests.pop_app().is_none(),
            "a frame with no input raises no app command"
        );
        if relayout == Relayout::Needed {
            ui.request_relayout();
        }
    }
}

/// A 6000 × 4000 frame of float RGB at `level`, one plane per channel: a stacked frame as lumos
/// publishes one.
fn stacked_frame(level: f32) -> DynamicValue {
    let size = Size2us::new(6000, 4000);
    let planes = std::array::from_fn(|_| Buffer2::new_filled(size, level));
    DynamicValue::from_custom(lens::Image::from(PlanarImage::new_rgb(planes)))
}

/// One editor-frame case: the made graph in `scenario`, painted on `backend`.
#[derive(Debug, Clone, Copy)]
struct FrameCase {
    scenario: Scenario,
    backend: Backend,
}

impl FrameCase {
    /// Measure the case: build the frame with `build` in the case, run it
    /// through [`SETTLE_FRAMES`], check its paint, then time frames.
    fn measure<F>(self, group: &mut BenchmarkGroup<'_, WallTime>, build: impl Fn() -> F)
    where
        F: FnMut(&mut Editor, u32) -> FramePaint,
    {
        let Self { scenario, backend } = self;
        group.bench_function(backend.case(scenario.label()), |b| {
            let mut frame = build();
            let mut editor = Editor::new();
            let mut n = 0u32;
            let mut step = || {
                let paint = frame(&mut editor, n);
                n = n.wrapping_add(1);
                paint
            };
            for _ in 0..SETTLE_FRAMES {
                step();
            }
            let paint = step();
            if scenario == Scenario::Cached {
                assert_eq!(paint, FramePaint::Skip, "a still editor paints nothing");
            } else {
                assert_ne!(
                    paint,
                    FramePaint::Skip,
                    "{} reaches paint",
                    scenario.label()
                );
            }
            b.iter(&mut step);
        });
    }
}

/// `integration/editor_frame/<backend>/<scenario>`: one whole editor frame of the made graph.
pub(crate) fn integration(c: &mut Criterion, backends: Backends) {
    let mut group = Kind::Integration.group(c, "editor_frame", Scale::Frame);
    for backend in backends.all() {
        for scenario in Scenario::ALL {
            let case = FrameCase { scenario, backend };
            match backend {
                Backend::Cpu => case.measure(&mut group, || {
                    let mut harness = UiHarness::with_text(BENCH_SURFACE).scale(BENCH_DPR);
                    harness.ui().set_theme(Theme::default().palantir);
                    let mut frontend = FrontendHarness::new(harness);
                    let mut published = false;
                    move |editor: &mut Editor, n: u32| {
                        let ui = frontend.harness().ui();
                        if !published {
                            editor.publish_previews(ui);
                            published = true;
                        }
                        editor.advance(scenario, ui, n);
                        frontend.frame(|ui| editor.record(ui)).paint()
                    }
                }),
                Backend::Gpu => case.measure(&mut group, || {
                    let gpu = BenchGpu::shared(Timing::PassOnly);
                    let target = gpu.target("darkroom.editor_frame", BENCH_SURFACE);
                    let mut host = OffscreenHost::builder(gpu.handles())
                        .collect_gpu_stats(false)
                        .retained_target(true)
                        .build();
                    host.ui().set_theme(Theme::default().palantir);
                    let mut published = false;
                    move |editor: &mut Editor, n: u32| {
                        if !published {
                            editor.publish_previews(host.ui());
                            published = true;
                        }
                        editor.advance(scenario, host.ui(), n);
                        let mut app = RecordApp::new(|ui: &mut Ui| editor.record(ui));
                        let paint = host.frame(target.as_target(), BENCH_DPR, &mut app).paint();
                        gpu.wait();
                        paint
                    }
                }),
            }
        }
    }
    group.finish();
}
