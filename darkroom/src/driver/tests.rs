use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{TempDir, fs};
use glam::{UVec2, Vec2};
use palantir::{DockView, Modifiers, PointerButton, Rect};
use scenarium::{Binding, ConstValue, InputPort, NodeId};
use tracing::{callsite, subscriber};
use tracing_subscriber::layer::SubscriberExt as _;

use crate::core::io::document as io_document;
use crate::core::io::document::document_format::DocumentFormat;
use crate::core::io::document::saved_paths::SavedPaths;
use crate::driver::action::{Action, Chord, DRAG_STEPS};
use crate::driver::error::{ActFailure, DriverError};
use crate::driver::locator::{Locator, Side};
use crate::driver::log_ring::LogRing;
use crate::driver::shot::{NODE_MARGIN, Region, Shot};
use crate::driver::state::{Detail, State};
use crate::driver::{COPY_LIMIT, DisplayKind, Driver, OpenMode, SessionConfig};
use crate::gui::dialogs::DialogAnswer;
use crate::gui::state::preview_store::StoredContent;
use crate::gui::theme::Theme;
use crate::internals::fixture;

const BRIGHTNESS: &str = "Brightness / Contrast";
const CONVERT: &str = "Convert";

fn driver() -> Driver {
    logged(LogRing::default())
}

/// [`driver`], whose logs `log` keeps.
fn logged(log: LogRing) -> Driver {
    Driver::new(
        SessionConfig {
            display: DisplayKind::Cpu,
            size: UVec2::new(1600, 1000),
            scale: 1.0,
        },
        log,
    )
    .unwrap()
}

/// A 400 × 300 window at scale 1 on `display`.
fn small(display: DisplayKind) -> Driver {
    Driver::new(
        SessionConfig {
            display,
            size: UVec2::new(400, 300),
            scale: 1.0,
        },
        LogRing::default(),
    )
    .unwrap()
}

fn decode(png: &[u8]) -> image::RgbaImage {
    image::load_from_memory(png).unwrap().to_rgba8()
}

fn srgb(color: palantir::RgbaF32) -> [u8; 4] {
    let bytes = color.to_srgba();
    [bytes.r, bytes.g, bytes.b, bytes.a]
}

fn click(at: Locator) -> Action {
    Action::Click {
        at,
        button: PointerButton::Left,
        mods: Modifiers::default(),
    }
}

fn drag(from: Locator, to: Locator) -> Action {
    Action::Drag {
        from,
        to,
        button: PointerButton::Left,
        mods: Modifiers::default(),
        steps: DRAG_STEPS,
    }
}

fn key(chord: &str) -> Action {
    Action::Key(Chord::parse(chord).unwrap())
}

fn node(name: &str) -> Locator {
    Locator::Node(name.to_string())
}

fn node_id(driver: &Driver, name: &str) -> NodeId {
    driver
        .app()
        .open_document()
        .document
        .graph
        .iter()
        .find(|node| node.name == name)
        .unwrap()
        .node_id
}

/// A click on a node selects it; a drag of its title moves it by the pointer's travel, exactly,
/// at the default zoom of 1; a drag from one node's output to the other's input wires them, a
/// filtered state keeps the nodes it names and the wire, and Ctrl+Z takes the wire back.
#[test]
fn clicks_drags_and_keys_edit_the_document() {
    let mut driver = driver();
    let dir = TempDir::new("darkroom-driver-edit");
    let path = fixture::project(dir.path(), &[BRIGHTNESS, CONVERT]);
    driver.open(&path, OpenMode::Copy).unwrap();

    let report = driver.act(&[click(node(BRIGHTNESS))]).unwrap();
    assert_eq!(report.selection, [BRIGHTNESS]);

    let id = node_id(&driver, BRIGHTNESS);
    let placed = |driver: &Driver| {
        driver
            .app()
            .open_document()
            .document
            .main_view
            .node_placements[&id]
            .pos
    };
    let before = placed(&driver);
    let header = Locator::NodeHeader(BRIGHTNESS.to_string());
    driver
        .act(&[drag(
            header.clone(),
            Locator::Offset(Box::new(header), Vec2::new(120.0, 60.0)),
        )])
        .unwrap();
    assert_eq!(placed(&driver), before + Vec2::new(120.0, 60.0));

    let wire = drag(
        Locator::Port {
            node: BRIGHTNESS.to_string(),
            side: Side::Out,
            port: "Image".to_string(),
        },
        Locator::Port {
            node: CONVERT.to_string(),
            side: Side::In,
            port: "0".to_string(),
        },
    );
    driver.act(&[wire]).unwrap();
    let wired = [format!("{BRIGHTNESS}.Image -> {CONVERT}.Image")];
    let detail = driver
        .state(Detail::Detailed, None)
        .unwrap()
        .details
        .unwrap();
    assert_eq!(detail.wires, wired);
    // A filter keeps the nodes it names in either form, and the wires with one at either end.
    let names = |state: &State| -> Vec<String> {
        state.nodes.iter().map(|node| node.name.clone()).collect()
    };
    let concise = driver.state(Detail::Concise, Some("Conv")).unwrap();
    assert_eq!(
        (names(&concise), concise.omitted),
        (vec![CONVERT.to_string()], 0)
    );
    for (filter, nodes) in [(BRIGHTNESS, [BRIGHTNESS]), ("Conv", [CONVERT])] {
        let state = driver.state(Detail::Detailed, Some(filter)).unwrap();
        assert_eq!(names(&state), nodes, "{filter}");
        assert_eq!(state.details.unwrap().wires, wired, "{filter}");
    }
    let none = driver.state(Detail::Detailed, Some("Blur")).unwrap();
    assert_eq!(names(&none), Vec::<String>::new());
    assert_eq!(none.details.unwrap().wires, Vec::<String>::new());

    driver.act(&[key("ctrl+z")]).unwrap();
    let detail = driver
        .state(Detail::Detailed, None)
        .unwrap()
        .details
        .unwrap();
    assert!(detail.wires.is_empty(), "undone: {:?}", detail.wires);
}

/// A world point resolves to the screen point where the canvas draws it: a node placed at its
/// world position has its body's corner there.
#[test]
fn a_world_point_lands_on_the_canvas_where_nodes_are_placed() {
    let mut driver = driver();
    let dir = TempDir::new("darkroom-driver-world");
    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    driver.open(&path, OpenMode::Copy).unwrap();
    let id = node_id(&driver, BRIGHTNESS);
    let pos = driver
        .app()
        .open_document()
        .document
        .main_view
        .node_placements[&id]
        .pos;
    let world = driver
        .offscreen()
        .with_app(|app, ui| Locator::World(pos).point(app, ui))
        .unwrap();
    let screen = driver.state(Detail::Concise, None).unwrap().nodes[0]
        .screen
        .unwrap();
    assert_eq!(world, Vec2::new(screen[0], screen[1]));
}

/// A right-click on the empty canvas opens the new-node palette; typed text ranks it, and Enter
/// adds the best match, which holds the focus.
#[test]
fn the_new_node_palette_adds_the_node_clicked() {
    let mut driver = driver();
    let dir = TempDir::new("darkroom-driver-palette");
    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    driver.open(&path, OpenMode::Copy).unwrap();
    driver
        .act(&[
            Action::Click {
                at: Locator::Screen(Vec2::new(1200.0, 800.0)),
                button: PointerButton::Right,
                mods: Modifiers::default(),
            },
            Action::Type("Convert".to_string()),
            key("enter"),
        ])
        .unwrap();
    let names: Vec<String> = driver
        .state(Detail::Concise, None)
        .unwrap()
        .nodes
        .into_iter()
        .map(|node| node.name)
        .collect();
    assert_eq!(names, [BRIGHTNESS, CONVERT], "by name");
}

/// Down moves the palette's focus to the next ranked match and Enter adds that one: "con"
/// ranks Convert (Image) before Concat (System), two names that start with it.
#[test]
fn down_then_enter_adds_the_next_ranked_match() {
    let mut driver = driver();
    let dir = TempDir::new("darkroom-driver-palette-keys");
    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    driver.open(&path, OpenMode::Copy).unwrap();
    driver
        .act(&[
            Action::Click {
                at: Locator::Screen(Vec2::new(1200.0, 800.0)),
                button: PointerButton::Right,
                mods: Modifiers::default(),
            },
            Action::Type("con".to_string()),
            key("down"),
            key("down"),
            key("up"),
            key("enter"),
        ])
        .unwrap();
    let names: Vec<String> = driver
        .state(Detail::Concise, None)
        .unwrap()
        .nodes
        .into_iter()
        .map(|node| node.name)
        .collect();
    assert_eq!(names, [BRIGHTNESS, "Concat"]);
}

/// A name no node has is not found, with the names there are; two nodes of one name are
/// ambiguous, with both ids, and `Nth` picks one of them; an index past them, and `Nth` over a
/// locator that cannot match several, are not found.
#[test]
fn a_node_locator_names_exactly_one_node() {
    let mut driver = driver();
    let dir = TempDir::new("darkroom-driver-names");
    let path = fixture::project(dir.path(), &[BRIGHTNESS, BRIGHTNESS]);
    driver.open(&path, OpenMode::Copy).unwrap();
    match driver.act(&[click(node("missing"))]) {
        Err(ActFailure {
            ran: 0,
            error: DriverError::NotFound { nearest, .. },
        }) => {
            assert_eq!(nearest, [BRIGHTNESS, BRIGHTNESS]);
        }
        other => panic!("{other:?}"),
    }
    match driver.act(&[click(node(BRIGHTNESS))]) {
        Err(ActFailure {
            ran: 0,
            error: DriverError::Ambiguous { matches, .. },
        }) => assert_eq!(matches.len(), 2),
        other => panic!("{other:?}"),
    }
    let nth = |locator: Locator, n: usize| click(Locator::Nth(Box::new(locator), n));
    let second = driver.act(&[nth(node(BRIGHTNESS), 1)]).unwrap();
    assert_eq!(second.selection, [BRIGHTNESS], "the second of the two");
    match driver.act(&[nth(node(BRIGHTNESS), 2)]) {
        Err(ActFailure {
            ran: 0,
            error: DriverError::NotFound { nearest, .. },
        }) => {
            assert!(nearest[0].ends_with("has 2 matches"));
        }
        other => panic!("{other:?}"),
    }
    match driver.act(&[nth(Locator::NodeHeader(BRIGHTNESS.to_string()), 0)]) {
        Err(ActFailure {
            ran: 0,
            error: DriverError::NotFound { nearest, .. },
        }) => {
            assert_eq!(nearest, ["Nth picks among Node or Text matches only"]);
        }
        other => panic!("{other:?}"),
    }
}

/// Two clicks on a node's title with a key between them stay two clicks: the second waits out
/// the double-click window whatever came between. A double click there opens its rename field;
/// the caret blinks there, and the session still settles with the field focused.
#[test]
fn a_focused_text_field_still_settles() {
    let mut driver = driver();
    let dir = TempDir::new("darkroom-driver-focus");
    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    driver.open(&path, OpenMode::Copy).unwrap();
    let header = || Locator::NodeHeader(BRIGHTNESS.to_string());
    let twice = driver
        .act(&[click(header()), key("escape"), click(header())])
        .unwrap();
    assert_ne!(
        twice.focus.as_deref(),
        Some(BRIGHTNESS),
        "no rename field opened"
    );

    let report = driver.act(&[Action::DoubleClick(header())]).unwrap();
    assert_eq!(
        report.focus.as_deref(),
        Some(BRIGHTNESS),
        "the rename field holds the focus"
    );
}

/// A copied session saves into its copy: the source stays byte for byte as it was. A folder over
/// the copy limit is refused, a single sparse file being enough to make one.
#[test]
fn a_copied_session_leaves_the_source_alone() {
    let mut driver = driver();
    let dir = TempDir::new("darkroom-driver-copy");
    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    let original = fs::read(&path).unwrap();
    driver.open(&path, OpenMode::Copy).unwrap();
    driver
        .act(&[
            drag(
                Locator::NodeHeader(BRIGHTNESS.to_string()),
                Locator::Screen(Vec2::new(900.0, 700.0)),
            ),
            key("ctrl+s"),
        ])
        .unwrap();
    let state = driver.state(Detail::Concise, None).unwrap();
    assert!(!state.dirty, "saved: {state:?}");
    let copy = PathBuf::from(state.document.unwrap());
    assert!(copy.starts_with(driver.session_dir()), "{}", copy.display());
    assert_ne!(fs::read(&copy).unwrap(), original, "the copy took the move");
    assert_eq!(
        fs::read(&path).unwrap(),
        original,
        "the source is as it was"
    );

    let filler = dir.join("filler.bin");
    fs::create(&filler)
        .unwrap()
        .set_len(COPY_LIMIT + 1)
        .unwrap();
    match driver.open(&path, OpenMode::Copy) {
        Err(DriverError::FolderTooLarge { bytes, .. }) => {
            assert_eq!(
                bytes,
                original.len() as u64 + COPY_LIMIT + 1,
                "the document and the filler"
            );
        }
        other => panic!("{other:?}"),
    }
}

/// A drag whose target is not found fails before any input goes in, so the next click finds no
/// button held; a pinch by no positive factor is refused, not dropped; and the error an unanswered dialog logs during `act` reaches the next `state`.
#[test]
fn a_failed_action_leaves_nothing_held_and_the_log_reaches_state() {
    let log = LogRing::default();
    let _tracing = subscriber::set_default(tracing_subscriber::registry().with(log.layer()));
    let mut driver = logged(log);
    let dir = TempDir::new("darkroom-driver-failed");
    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    driver.open(&path, OpenMode::Copy).unwrap();
    assert!(matches!(
        driver.act(&[drag(node(BRIGHTNESS), node("missing"))]),
        Err(ActFailure {
            ran: 0,
            error: DriverError::NotFound { .. }
        })
    ));
    let report = driver.act(&[click(node(BRIGHTNESS))]).unwrap();
    assert_eq!(report.selection, [BRIGHTNESS]);
    for factor in [0.0, -2.0] {
        assert!(matches!(
            driver.act(&[Action::Pinch {
                at: node(BRIGHTNESS),
                factor
            }]),
            Err(ActFailure { ran: 0, error: DriverError::BadPinch { factor: refused } }) if refused == factor
        ));
    }

    // tracing-core caches each callsite's interest globally, and another test thread that hits
    // the dialog's callsite can leave it cached as never for this thread's scoped subscriber:
    // six of eight full runs dropped the line without this rebuild, none of eight with it.
    callsite::rebuild_interest_cache();
    driver.act(&[key("ctrl+o")]).unwrap();
    let log = driver.state(Detail::Concise, None).unwrap().log;
    assert_eq!(
        log,
        [
            "ERROR darkroom::gui::dialogs: a dialog asked to open a project, and no queued answer was left"
        ]
    );
}

/// A copy that would break the project is refused: a document whose relative path climbs out of
/// its folder, which would name another file beside the copy, and a link to a folder inside the
/// project, which could copy the folder into itself. In place, the first opens, and a document
/// that does not load is refused with the editor's status line. A refused open leaves the editor
/// on the project it had; a new document has no file and no nodes.
#[test]
fn a_folder_a_copy_would_break_is_refused() {
    let mut driver = driver();
    let dir = TempDir::new("darkroom-driver-break");
    let path = fixture::project(dir.path(), &["Load Image"]);
    let mut document = io_document::load(&path).unwrap().document;
    let node_id = document.graph.iter().next().unwrap().node_id;
    document.graph.bindings.insert(
        InputPort::new(node_id, 0),
        Binding::Const(ConstValue::FsPath("../x.tiff".to_string())),
    );
    io_document::internals::write_unchecked(&document, &path);
    match driver.open(&path, OpenMode::Copy) {
        Err(DriverError::PathsLeaveFolder { paths, .. }) => assert_eq!(paths, ["../x.tiff"]),
        other => panic!("{other:?}"),
    }
    driver.open(&path, OpenMode::InPlace).unwrap();
    let first = path;

    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    fs::symlink_dir(dir.path(), dir.join("loop")).unwrap();
    match driver.open(&path, OpenMode::Copy) {
        Err(DriverError::LinkedFolder { path }) => assert_eq!(path, dir.join("loop")),
        other => panic!("{other:?}"),
    }

    let missing = dir.join("missing.darkroom");
    match driver.open(&missing, OpenMode::InPlace) {
        Err(DriverError::NotOpened {
            path,
            status: Some(status),
        }) => {
            assert_eq!(path, missing);
            assert!(status.starts_with("load failed: "), "{status}");
        }
        other => panic!("{other:?}"),
    }
    let state = driver.state(Detail::Concise, None).unwrap();
    assert_eq!(
        state.document.as_deref(),
        first.to_str(),
        "the refused opens leave the editor on the one that opened"
    );

    driver.new_document().unwrap();
    let state = driver.state(Detail::Concise, None).unwrap();
    assert_eq!((state.document, state.nodes.len()), (None, 0));
}

/// A shot of the whole 400 × 300 window is the frame: the canvas's ground and the node's body
/// show the theme's colours exactly, and the legend's node rect is the rect `state` reports, as
/// the scale is 1. A `max_side` of 100 scales it to 100 × 75 and the legend by 1/4, exactly. A
/// node's shot is its body, (40, 100) to (303.5, 245), with 24 px around it, clamped to the
/// window and rounded out: (16, 76) to (328, 269), 312 × 193, the body at
/// (24, 24) in it. A rect of the canvas at the node's place, 100 × 50 at zoom 1, starts at the
/// body's corner, and one past the window or of negative size is empty. A pane's shot is its rect, rounded out; a name no visible tab shows lists those
/// that are. The CPU display lays the node out where the GPU one does, and has no shots.
#[test]
fn a_shot_is_the_frame_with_a_legend_of_what_it_shows() {
    let mut driver = small(DisplayKind::TestGpu);
    let dir = TempDir::new("darkroom-driver-shot");
    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    driver.open(&path, OpenMode::Copy).unwrap();
    let screen = driver.state(Detail::Concise, None).unwrap().nodes[0]
        .screen
        .unwrap();
    assert_eq!(screen, [40.0, 100.0, 263.4953, 145.0]);

    let theme = Theme::default();
    let whole = driver.shot(&Region::Window, 400).unwrap();
    assert_eq!(whole.size, UVec2::new(400, 300));
    assert_eq!(whole.legend[0].name, BRIGHTNESS);
    assert_eq!(whole.legend[0].rect, screen);
    let pixels = decode(&whole.png);
    assert_eq!(
        pixels.get_pixel(380, 90).0,
        srgb(theme.canvas.bg),
        "the canvas"
    );
    assert_eq!(
        pixels.get_pixel(250, 240).0,
        srgb(theme.card.fill),
        "the body"
    );
    assert_eq!(fs::read(&whole.path).unwrap(), whole.png);
    assert!(
        whole.path.ends_with("shots/001-window.png"),
        "{}",
        whole.path.display()
    );

    let quarter = driver.shot(&Region::Window, 100).unwrap();
    assert_eq!(quarter.size, UVec2::new(100, 75));
    assert_eq!(decode(&quarter.png).dimensions(), (100, 75));
    assert_eq!(quarter.legend[0].rect, screen.map(|v| v / 4.0));
    let ports = |shot: &Shot| {
        shot.legend[0]
            .inputs
            .iter()
            .chain(&shot.legend[0].outputs)
            .map(|port| (port.name.clone(), port.rect))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ports(&quarter),
        ports(&whole)
            .into_iter()
            .map(|(name, rect)| (name, rect.map(|v| v / 4.0)))
            .collect::<Vec<_>>()
    );

    let node = driver
        .shot(&Region::Node(BRIGHTNESS.to_string()), 1000)
        .unwrap();
    assert_eq!(NODE_MARGIN, 24.0);
    assert_eq!(node.size, UVec2::new(312, 193));
    assert_eq!(node.legend[0].rect[..2], [24.0, 24.0]);
    assert!(
        node.path
            .ends_with("shots/003-node-brightness---contrast.png")
    );

    let id = node_id(&driver, BRIGHTNESS);
    let placed = driver
        .app()
        .open_document()
        .document
        .main_view
        .node_placements[&id]
        .pos;
    let world = driver
        .shot(
            &Region::World(Rect::new(placed.x, placed.y, 100.0, 50.0)),
            1000,
        )
        .unwrap();
    assert_eq!(world.size, UVec2::new(100, 50));
    assert_eq!(world.legend[0].rect, [0.0, 0.0, 100.0, 50.0]);
    assert!(matches!(
        driver.shot(&Region::World(Rect::new(1e4, 1e4, 10.0, 10.0)), 1000),
        Err(DriverError::EmptyRegion { .. })
    ));
    assert!(matches!(
        driver.shot(
            &Region::World(Rect::new(placed.x, placed.y, -10.0, 10.0)),
            1000
        ),
        Err(DriverError::EmptyRegion { .. })
    ));
    assert!(
        driver
            .act(&[click(Locator::Node(BRIGHTNESS.to_string()))])
            .is_ok(),
        "settled after the failure"
    );

    let rect = driver
        .offscreen()
        .with_app(|app, ui| {
            let layout = &app.open_document().document.layout;
            ui.response_for(DockView::pane_id(layout, layout.primary().id()))
                .rect
        })
        .unwrap();
    let pane = driver
        .shot(&Region::Pane("Graph".to_string()), 1000)
        .unwrap();
    let rounded = rect.max().ceil() - rect.min.floor();
    assert_eq!(pane.size.as_vec2(), rounded, "{rect:?}");
    match driver.shot(&Region::Pane("missing".to_string()), 1000) {
        Err(DriverError::NotFound { nearest, .. }) => assert_eq!(nearest, ["Graph"]),
        other => panic!("{other:?}"),
    }

    let mut cpu = small(DisplayKind::Cpu);
    cpu.open(&path, OpenMode::Copy).unwrap();
    assert_eq!(
        cpu.state(Detail::Concise, None).unwrap().nodes[0].screen,
        Some(screen)
    );
    assert!(matches!(
        cpu.shot(&Region::Window, 100),
        Err(DriverError::NoGpu)
    ));
}

/// View ▸ Copy View as Image puts an image on the clipboard, which then holds no text; View ▸
/// Save View as PNG… writes the scripted answer with the extension added, and the file holds the
/// window as a shot of it shows it.
#[test]
fn the_view_menu_copies_and_saves_the_window() {
    let mut driver = small(DisplayKind::TestGpu);
    let dir = TempDir::new("darkroom-driver-view");
    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    driver.open(&path, OpenMode::Copy).unwrap();
    let view = || click(Locator::Text("View".to_string()));

    let clipboard = driver.offscreen().with_app(|_, ui| ui.clipboard());
    clipboard.set_text("text").unwrap();
    driver
        .act(&[
            view(),
            click(Locator::Text("Copy View as Image".to_string())),
        ])
        .unwrap();
    assert_eq!(clipboard.text().unwrap(), "");

    driver
        .act(&[
            Action::Answer(DialogAnswer::Path(dir.join("view"))),
            view(),
            click(Locator::Text("Save View as PNG…".to_string())),
        ])
        .unwrap();
    let state = driver.state(Detail::Concise, None).unwrap();
    assert_eq!((state.status, state.log), (None, Vec::<String>::new()));
    let saved = decode(&fs::read(dir.join("view.png")).unwrap());
    let shot = driver.shot(&Region::Window, 400).unwrap();
    assert_eq!(saved, decode(&shot.png));
}

/// A save keeps the format the file was opened in, Save As writes plain RON, and Save As
/// Compressed… writes it zipped: a zipped project saves zipped, Ctrl+Shift+S makes a plain copy,
/// and the menu row makes a zipped one again.
#[test]
fn a_save_keeps_the_file_format_and_save_as_names_one() {
    let mut driver = driver();
    let dir = TempDir::new("darkroom-driver-format");
    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    let file = io_document::load(&path).unwrap();
    io_document::save(
        &file.document,
        &path,
        SavedPaths::Relative,
        DocumentFormat::ZippedRon,
    )
    .unwrap();
    driver.open(&path, OpenMode::InPlace).unwrap();
    let zipped = |path: &Path| fs::read(path).unwrap().starts_with(b"PK");

    driver
        .act(&[
            drag(
                Locator::NodeHeader(BRIGHTNESS.to_string()),
                Locator::Screen(Vec2::new(900.0, 700.0)),
            ),
            key("ctrl+s"),
        ])
        .unwrap();
    assert!(zipped(&path), "the save kept the zipped format");

    let plain = dir.join("plain.darkroom");
    driver
        .act(&[
            Action::Answer(DialogAnswer::Path(plain.clone())),
            key("ctrl+shift+s"),
        ])
        .unwrap();
    assert!(!zipped(&plain), "Save As writes plain RON");

    let again = dir.join("again.darkroom");
    driver
        .act(&[
            Action::Answer(DialogAnswer::Path(again.clone())),
            click(Locator::Text("File".to_string())),
            click(Locator::Text("Save As Compressed…".to_string())),
        ])
        .unwrap();
    assert!(zipped(&again), "Save As Compressed writes it zipped");
    assert_eq!(
        io_document::load(&again).unwrap().document,
        io_document::load(&plain).unwrap().document
    );
}

/// The sample project runs to completion, and each preview shows an image of its input's size:
/// the TIFF's 895 × 551 after Brightness / Contrast and after Blend, and the FITS frame's
/// 100 × 100 after Auto Stretch.
#[test]
fn the_sample_project_runs_to_its_previews() {
    let mut driver = driver();
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("sample/sample.darkroom");
    driver.open(&sample, OpenMode::Copy).unwrap();
    // A run that ends while a later action of the batch plays is still the batch's run.
    driver
        .act(&[key("ctrl+r"), Action::Move(Locator::Screen(Vec2::ZERO))])
        .unwrap();
    driver.wait_run(Duration::from_secs(30)).unwrap();

    let state = driver.state(Detail::Detailed, None).unwrap();
    for node in &state.nodes {
        assert!(
            node.run
                .as_ref()
                .is_some_and(|run| run.starts_with("Executed")),
            "{}: {:?}",
            node.name,
            node.run
        );
    }
    let mut sizes: Vec<(String, UVec2)> = state
        .nodes
        .iter()
        .filter(|node| node.name.starts_with("Preview"))
        .map(|node| {
            let shown = driver
                .app()
                .run_state()
                .previews()
                .get(node_id(&driver, &node.name))
                .and_then(StoredContent::image)
                .unwrap_or_else(|| panic!("{} shows an image", node.name));
            (node.name.clone(), shown.preview.native_size)
        })
        .collect();
    sizes.sort_by(|a, b| a.0.cmp(&b.0));
    let lena = UVec2::new(895, 551);
    assert_eq!(
        sizes,
        [
            ("Preview Adjusted".to_string(), lena),
            ("Preview Astro".to_string(), UVec2::new(100, 100)),
            ("Preview Blend".to_string(), lena),
        ]
    );
}

/// A quit ends the session as it closes a window. With unsaved edits Ctrl+Q only asks, and
/// Cancel keeps the session; Don't Save then quits, and every later call that runs a frame is
/// refused, the action after it unplayed. A new open starts afresh, and with no person to own
/// them, discards unsaved edits.
#[test]
fn a_quit_ends_the_session() {
    let mut driver = driver();
    let dir = TempDir::new("darkroom-driver-quit");
    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    driver.open(&path, OpenMode::Copy).unwrap();
    let quit = || key("ctrl+q");
    driver
        .act(&[
            drag(
                Locator::NodeHeader(BRIGHTNESS.to_string()),
                Locator::Screen(Vec2::new(900.0, 700.0)),
            ),
            quit(),
            click(Locator::Text("Cancel".to_string())),
        ])
        .unwrap();
    assert!(
        driver.state(Detail::Concise, None).unwrap().dirty,
        "still open, still dirty"
    );

    match driver.act(&[
        quit(),
        click(Locator::Text("Don't Save".to_string())),
        click(node(BRIGHTNESS)),
    ]) {
        Err(ActFailure {
            ran: 1,
            error: DriverError::Quit,
        }) => {}
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        driver.act(&[]),
        Err(ActFailure {
            ran: 0,
            error: DriverError::Quit
        })
    ));
    assert!(matches!(
        driver.wait_run(Duration::from_secs(1)),
        Err(DriverError::Quit)
    ));

    driver.open(&path, OpenMode::Copy).unwrap();
    let report = driver.act(&[click(node(BRIGHTNESS))]).unwrap();
    assert_eq!(report.selection, [BRIGHTNESS]);

    let report = driver
        .act(&[drag(
            Locator::NodeHeader(BRIGHTNESS.to_string()),
            Locator::Screen(Vec2::new(900.0, 700.0)),
        )])
        .unwrap();
    assert!(report.dirty);
    driver.new_document().unwrap();
    let state = driver.state(Detail::Concise, None).unwrap();
    assert_eq!(
        (state.document, state.dirty),
        (None, false),
        "offscreen, an open discards unsaved edits"
    );
}

/// A wire dropped on bare canvas opens the palette, and a pick adds the node and floats the wire
/// again, bound to nothing until a click on the new node's input lands it.
#[test]
fn a_dropped_wire_floats_onto_the_node_its_palette_adds() {
    let mut driver = driver();
    let dir = TempDir::new("darkroom-driver-drop");
    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    driver.open(&path, OpenMode::Copy).unwrap();
    let output = Locator::Port {
        node: BRIGHTNESS.to_string(),
        side: Side::Out,
        port: "Image".to_string(),
    };
    driver
        .act(&[drag(output, Locator::Screen(Vec2::new(1100.0, 700.0)))])
        .unwrap();
    // Picked by the keys: a row's text joins its name, category and description, and the
    // library may scroll Convert out of view.
    driver
        .act(&[Action::Type(CONVERT.to_string()), key("enter")])
        .unwrap();
    let wires = |driver: &mut Driver| {
        driver
            .state(Detail::Detailed, None)
            .unwrap()
            .details
            .unwrap()
            .wires
    };
    let names: Vec<String> = driver
        .state(Detail::Concise, None)
        .unwrap()
        .nodes
        .into_iter()
        .map(|node| node.name)
        .collect();
    assert_eq!(names, [BRIGHTNESS, CONVERT], "the pick added the node");
    assert_eq!(wires(&mut driver), Vec::<String>::new(), "the wire floats");

    driver
        .act(&[click(Locator::Port {
            node: CONVERT.to_string(),
            side: Side::In,
            port: "0".to_string(),
        })])
        .unwrap();
    assert_eq!(
        wires(&mut driver),
        [format!("{BRIGHTNESS}.Image -> {CONVERT}.Image")]
    );
}
