use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{TempDir, fs};
use glam::{UVec2, Vec2};
use palantir::{DockView, Modifiers, PointerButton, Rect};
use scenarium::{Binding, ConstValue, InputPort, NodeId};

use crate::core::io::document as io_document;
use crate::core::io::document::document_format::DocumentFormat;
use crate::core::io::document::saved_paths::SavedPaths;
use crate::gui::state::preview_store::StoredContent;
use crate::gui::theme::Theme;
use crate::internals::harness::action::{Action, DRAG_STEPS, DialogAnswer};
use crate::internals::harness::error::HarnessError;
use crate::internals::harness::fixture;
use crate::internals::harness::locator::{Locator, Side};
use crate::internals::harness::shot::{NODE_MARGIN, Region, Shot};
use crate::internals::harness::state::Detail;
use crate::internals::harness::{COPY_LIMIT, DisplayKind, Harness, SessionConfig};

const BRIGHTNESS: &str = "Brightness / Contrast";
const CONVERT: &str = "Convert";

fn harness() -> Harness {
    Harness::new(SessionConfig {
        display: DisplayKind::Cpu,
        size: UVec2::new(1600, 1000),
        scale: 1.0,
    })
}

/// A 400 × 300 window at scale 1 on `display`.
fn small(display: DisplayKind) -> Harness {
    Harness::new(SessionConfig {
        display,
        size: UVec2::new(400, 300),
        scale: 1.0,
    })
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

fn node(name: &str) -> Locator {
    Locator::Node(name.to_string())
}

fn node_id(harness: &Harness, name: &str) -> NodeId {
    harness
        .app
        .open_document()
        .document
        .graph
        .iter()
        .find(|node| node.name == name)
        .unwrap()
        .node_id
}

/// A click on a node selects it; a drag of its title moves it by the pointer's travel, exactly,
/// at the default zoom of 1; a drag from one node's output to the other's input wires them, and
/// Ctrl+Z takes the wire back.
#[test]
fn clicks_drags_and_keys_edit_the_document() {
    let mut harness = harness();
    let dir = TempDir::new("darkroom-harness-edit");
    let path = fixture::project(dir.path(), &[BRIGHTNESS, CONVERT]);
    harness.open(&path, false).unwrap();

    let report = harness.act(&[click(node(BRIGHTNESS))]).unwrap();
    assert_eq!(report.selection, [BRIGHTNESS]);

    let id = node_id(&harness, BRIGHTNESS);
    let placed = |harness: &Harness| {
        harness
            .app
            .open_document()
            .document
            .main_view
            .node_placements[&id]
            .pos
    };
    let before = placed(&harness);
    let header = Locator::NodeHeader(BRIGHTNESS.to_string());
    harness
        .act(&[drag(
            header.clone(),
            Locator::Offset(Box::new(header), Vec2::new(120.0, 60.0)),
        )])
        .unwrap();
    assert_eq!(placed(&harness), before + Vec2::new(120.0, 60.0));

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
    harness.act(&[wire]).unwrap();
    let detail = harness.state(Detail::Detailed, None).details.unwrap();
    assert_eq!(detail.wires.len(), 1, "{:?}", detail.wires);
    assert!(
        detail.wires[0].starts_with(&format!("{BRIGHTNESS}.Image -> {CONVERT}.")),
        "{:?}",
        detail.wires
    );

    harness.act(&[Action::Key("ctrl+z".to_string())]).unwrap();
    let detail = harness.state(Detail::Detailed, None).details.unwrap();
    assert!(detail.wires.is_empty(), "undone: {:?}", detail.wires);
}

/// A world point resolves to the screen point where the canvas draws it: a node placed at its
/// world position has its body's corner there.
#[test]
fn a_world_point_lands_on_the_canvas_where_nodes_are_placed() {
    let mut harness = harness();
    let dir = TempDir::new("darkroom-harness-world");
    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    harness.open(&path, false).unwrap();
    let id = node_id(&harness, BRIGHTNESS);
    let pos = harness
        .app
        .open_document()
        .document
        .main_view
        .node_placements[&id]
        .pos;
    let world = Locator::World(pos)
        .point(&harness.app, harness.display.ui())
        .unwrap();
    let screen = harness.state(Detail::Concise, None).nodes[0]
        .screen
        .unwrap();
    assert_eq!(world, Vec2::new(screen[0], screen[1]));
}

/// A right-click on the empty canvas opens the new-node palette; typed text filters it, and a
/// click on the func's row adds that node.
#[test]
fn the_new_node_palette_adds_the_node_clicked() {
    let mut harness = harness();
    let dir = TempDir::new("darkroom-harness-palette");
    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    harness.open(&path, false).unwrap();
    harness
        .act(&[
            Action::Click {
                at: Locator::Screen(Vec2::new(1200.0, 800.0)),
                button: PointerButton::Right,
                mods: Modifiers::default(),
            },
            Action::Type("Convert".to_string()),
            // The search field shows the typed text too; the row is the second match.
            click(Locator::Nth(
                Box::new(Locator::Text(CONVERT.to_string())),
                1,
            )),
        ])
        .unwrap();
    let names: Vec<String> = harness
        .state(Detail::Concise, None)
        .nodes
        .into_iter()
        .map(|node| node.name)
        .collect();
    assert!(names.contains(&CONVERT.to_string()), "{names:?}");
}

/// A name no node has is not found, with the names there are; two nodes of one name are
/// ambiguous, with both ids, and `Nth` picks one of them; an index past them, and `Nth` over a
/// locator that cannot match several, are not found.
#[test]
fn a_node_locator_names_exactly_one_node() {
    let mut harness = harness();
    let dir = TempDir::new("darkroom-harness-names");
    let path = fixture::project(dir.path(), &[BRIGHTNESS, BRIGHTNESS]);
    harness.open(&path, false).unwrap();
    match harness.act(&[click(node("missing"))]) {
        Err(HarnessError::NotFound { nearest, .. }) => {
            assert_eq!(nearest, [BRIGHTNESS, BRIGHTNESS]);
        }
        other => panic!("{other:?}"),
    }
    match harness.act(&[click(node(BRIGHTNESS))]) {
        Err(HarnessError::Ambiguous { matches, .. }) => assert_eq!(matches.len(), 2),
        other => panic!("{other:?}"),
    }
    let nth = |locator: Locator, n: usize| click(Locator::Nth(Box::new(locator), n));
    let second = harness.act(&[nth(node(BRIGHTNESS), 1)]).unwrap();
    assert_eq!(second.selection, [BRIGHTNESS], "the second of the two");
    match harness.act(&[nth(node(BRIGHTNESS), 2)]) {
        Err(HarnessError::NotFound { nearest, .. }) => {
            assert!(nearest[0].ends_with("has 2 matches"));
        }
        other => panic!("{other:?}"),
    }
    match harness.act(&[nth(Locator::NodeHeader(BRIGHTNESS.to_string()), 0)]) {
        Err(HarnessError::NotFound { nearest, .. }) => {
            assert_eq!(nearest, ["Nth picks among Node or Text matches only"]);
        }
        other => panic!("{other:?}"),
    }
}

/// A double click on a node's title opens its rename field; the caret blinks there, and the
/// session still settles with the field focused.
#[test]
fn a_focused_text_field_still_settles() {
    let mut harness = harness();
    let dir = TempDir::new("darkroom-harness-focus");
    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    harness.open(&path, false).unwrap();
    let report = harness
        .act(&[Action::DoubleClick(Locator::NodeHeader(
            BRIGHTNESS.to_string(),
        ))])
        .unwrap();
    assert!(report.focus.is_some(), "the rename field holds the focus");
}

/// A copied session saves into its copy: the source stays byte for byte as it was. A folder over
/// the copy limit is refused, a single sparse file being enough to make one.
#[test]
fn a_copied_session_leaves_the_source_alone() {
    let mut harness = harness();
    let dir = TempDir::new("darkroom-harness-copy");
    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    let original = fs::read(&path).unwrap();
    harness.open(&path, false).unwrap();
    harness
        .act(&[
            drag(
                Locator::NodeHeader(BRIGHTNESS.to_string()),
                Locator::Screen(Vec2::new(900.0, 700.0)),
            ),
            Action::Key("ctrl+s".to_string()),
        ])
        .unwrap();
    let state = harness.state(Detail::Concise, None);
    assert!(!state.dirty, "saved: {state:?}");
    let copy = PathBuf::from(state.document.unwrap());
    assert!(
        copy.starts_with(harness.session_dir()),
        "{}",
        copy.display()
    );
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
    match harness.open(&path, false) {
        Err(HarnessError::FolderTooLarge { bytes, .. }) => assert!(bytes > COPY_LIMIT),
        other => panic!("{other:?}"),
    }
}

/// A drag whose target is not found fails before any input goes in, so the next click finds no
/// button held; a pinch by no positive factor is refused, not dropped; and the error an unanswered dialog logs during `act` reaches the next `state`.
#[test]
fn a_failed_action_leaves_nothing_held_and_the_log_reaches_state() {
    let mut harness = harness();
    let dir = TempDir::new("darkroom-harness-failed");
    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    harness.open(&path, false).unwrap();
    assert!(matches!(
        harness.act(&[drag(node(BRIGHTNESS), node("missing"))]),
        Err(HarnessError::NotFound { .. })
    ));
    let report = harness.act(&[click(node(BRIGHTNESS))]).unwrap();
    assert_eq!(report.selection, [BRIGHTNESS]);
    for factor in [0.0, -2.0] {
        assert!(matches!(
            harness.act(&[Action::Pinch {
                at: node(BRIGHTNESS),
                factor
            }]),
            Err(HarnessError::BadPinch { factor: refused }) if refused == factor
        ));
    }

    harness.act(&[Action::Key("ctrl+o".to_string())]).unwrap();
    let log = harness.state(Detail::Concise, None).log;
    assert!(
        log.iter()
            .any(|line| line.contains("no scripted answer was left")),
        "{log:?}"
    );
}

/// A copy that would break the project is refused: a document whose relative path climbs out of
/// its folder, which would name another file beside the copy, and a link to a folder inside the
/// project, which could copy the folder into itself. In place, the first opens.
#[test]
fn a_folder_a_copy_would_break_is_refused() {
    let mut harness = harness();
    let dir = TempDir::new("darkroom-harness-break");
    let path = fixture::project(dir.path(), &["Load Image"]);
    let mut document = io_document::load(&path).unwrap().document;
    let node_id = document.graph.iter().next().unwrap().node_id;
    document.graph.bindings.insert(
        InputPort::new(node_id, 0),
        Binding::Const(ConstValue::FsPath("../x.tiff".to_string())),
    );
    io_document::internals::write_unchecked(&document, &path);
    match harness.open(&path, false) {
        Err(HarnessError::PathsLeaveFolder { paths, .. }) => assert_eq!(paths, ["../x.tiff"]),
        other => panic!("{other:?}"),
    }
    harness.open(&path, true).unwrap();

    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    fs::symlink(dir.path(), dir.join("loop")).unwrap();
    assert!(matches!(
        harness.open(&path, false),
        Err(HarnessError::LinkedFolder { .. })
    ));
}

/// A shot of the whole 400 × 300 window is the frame: the canvas's ground and the node's body
/// show the theme's colours exactly, and the legend's node rect is the rect `state` reports, as
/// the scale is 1. A `max_side` of 100 scales it to 100 × 75 and the legend by 1/4, exactly. A
/// node's shot is its body, clipped by the canvas to (40, 100) to (327.5, 278), with 24 px around
/// it, clamped to the window and rounded out: (16, 76) to (352, 300), 336 × 224, the body at
/// (24, 24) in it. A rect of the canvas at the node's place, 100 × 50 at zoom 1, starts at the
/// body's corner, and one past the window or of negative size is empty. A pane's shot is its rect, rounded out; a name no visible tab shows lists those
/// that are. The CPU display lays the node out where the GPU one does, and has no shots.
#[test]
fn a_shot_is_the_frame_with_a_legend_of_what_it_shows() {
    let mut harness = small(DisplayKind::Gpu);
    let dir = TempDir::new("darkroom-harness-shot");
    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    harness.open(&path, false).unwrap();
    let screen = harness.state(Detail::Concise, None).nodes[0]
        .screen
        .unwrap();
    assert_eq!(screen, [40.0, 100.0, 287.4953, 178.0]);

    let theme = Theme::default();
    let whole = harness.shot(&Region::Window, 400).unwrap();
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

    let quarter = harness.shot(&Region::Window, 100).unwrap();
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

    let node = harness
        .shot(&Region::Node(BRIGHTNESS.to_string()), 1000)
        .unwrap();
    assert_eq!(NODE_MARGIN, 24.0);
    assert_eq!(node.size, UVec2::new(336, 224));
    assert_eq!(node.legend[0].rect[..2], [24.0, 24.0]);
    assert!(
        node.path
            .ends_with("shots/003-node-brightness---contrast.png")
    );

    let id = node_id(&harness, BRIGHTNESS);
    let placed = harness
        .app
        .open_document()
        .document
        .main_view
        .node_placements[&id]
        .pos;
    let world = harness
        .shot(
            &Region::World(Rect::new(placed.x, placed.y, 100.0, 50.0)),
            1000,
        )
        .unwrap();
    assert_eq!(world.size, UVec2::new(100, 50));
    assert_eq!(world.legend[0].rect, [0.0, 0.0, 100.0, 50.0]);
    assert!(matches!(
        harness.shot(&Region::World(Rect::new(1e4, 1e4, 10.0, 10.0)), 1000),
        Err(HarnessError::EmptyRegion { .. })
    ));
    assert!(matches!(
        harness.shot(
            &Region::World(Rect::new(placed.x, placed.y, -10.0, 10.0)),
            1000
        ),
        Err(HarnessError::EmptyRegion { .. })
    ));
    assert!(
        harness
            .act(&[click(Locator::Node(BRIGHTNESS.to_string()))])
            .is_ok(),
        "settled after the failure"
    );

    let layout = &harness.app.open_document().document.layout;
    let rect = harness
        .display
        .ui()
        .response_for(DockView::pane_id(layout, layout.primary().id))
        .rect
        .unwrap();
    let pane = harness
        .shot(&Region::Pane("main".to_string()), 1000)
        .unwrap();
    let rounded = rect.max().ceil() - rect.min.floor();
    assert_eq!(pane.size.as_vec2(), rounded, "{rect:?}");
    match harness.shot(&Region::Pane("missing".to_string()), 1000) {
        Err(HarnessError::NotFound { nearest, .. }) => assert_eq!(nearest, ["main"]),
        other => panic!("{other:?}"),
    }

    let mut cpu = small(DisplayKind::Cpu);
    cpu.open(&path, false).unwrap();
    assert_eq!(
        cpu.state(Detail::Concise, None).nodes[0].screen,
        Some(screen)
    );
    assert!(matches!(
        cpu.shot(&Region::Window, 100),
        Err(HarnessError::NoGpu)
    ));
}

/// View ▸ Copy View as Image puts an image on the clipboard, which then holds no text; View ▸
/// Save View as PNG… writes the scripted answer with the extension added, and the file holds the
/// window as a shot of it shows it.
#[test]
fn the_view_menu_copies_and_saves_the_window() {
    let mut harness = small(DisplayKind::Gpu);
    let dir = TempDir::new("darkroom-harness-view");
    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    harness.open(&path, false).unwrap();
    let view = || click(Locator::Text("View".to_string()));

    let clipboard = harness.display.ui().clipboard();
    clipboard.set_text("text").unwrap();
    harness
        .act(&[
            view(),
            click(Locator::Text("Copy View as Image".to_string())),
        ])
        .unwrap();
    assert_eq!(clipboard.text().unwrap(), "");

    harness
        .act(&[
            Action::Answer(DialogAnswer::Path(dir.join("view"))),
            view(),
            click(Locator::Text("Save View as PNG…".to_string())),
        ])
        .unwrap();
    let state = harness.state(Detail::Concise, None);
    assert_eq!((state.status, state.log), (None, Vec::<String>::new()));
    let saved = decode(&fs::read(dir.join("view.png")).unwrap());
    let shot = harness.shot(&Region::Window, 400).unwrap();
    assert_eq!(saved, decode(&shot.png));
}

/// A save keeps the format the file was opened in, Save As writes plain RON, and Save As
/// Compressed… writes it zipped: a zipped project saves zipped, Ctrl+Shift+S makes a plain copy,
/// and the menu row makes a zipped one again.
#[test]
fn a_save_keeps_the_file_format_and_save_as_names_one() {
    let mut harness = harness();
    let dir = TempDir::new("darkroom-harness-format");
    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    let file = io_document::load(&path).unwrap();
    io_document::save(
        &file.document,
        &path,
        SavedPaths::Relative,
        DocumentFormat::ZippedRon,
    )
    .unwrap();
    harness.open(&path, true).unwrap();
    let zipped = |path: &Path| fs::read(path).unwrap().starts_with(b"PK");

    harness
        .act(&[
            drag(
                Locator::NodeHeader(BRIGHTNESS.to_string()),
                Locator::Screen(Vec2::new(900.0, 700.0)),
            ),
            Action::Key("ctrl+s".to_string()),
        ])
        .unwrap();
    assert!(zipped(&path), "the save kept the zipped format");

    let plain = dir.join("plain.darkroom");
    harness
        .act(&[
            Action::Answer(DialogAnswer::Path(plain.clone())),
            Action::Key("ctrl+shift+s".to_string()),
        ])
        .unwrap();
    assert!(!zipped(&plain), "Save As writes plain RON");

    let again = dir.join("again.darkroom");
    harness
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
    let mut harness = harness();
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("sample/sample.darkroom");
    harness.open(&sample, false).unwrap();
    harness.act(&[Action::Key("ctrl+r".to_string())]).unwrap();
    harness.wait_run(Duration::from_secs(30)).unwrap();

    let state = harness.state(Detail::Detailed, None);
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
            let shown = harness
                .app
                .run_state()
                .previews
                .get(node_id(&harness, &node.name))
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
/// refused, the action after it unplayed. A new open starts afresh.
#[test]
fn a_quit_ends_the_session() {
    let mut harness = harness();
    let dir = TempDir::new("darkroom-harness-quit");
    let path = fixture::project(dir.path(), &[BRIGHTNESS]);
    harness.open(&path, false).unwrap();
    let quit = || Action::Key("ctrl+q".to_string());
    harness
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
        harness.state(Detail::Concise, None).dirty,
        "still open, still dirty"
    );

    match harness.act(&[
        quit(),
        click(Locator::Text("Don't Save".to_string())),
        click(node(BRIGHTNESS)),
    ]) {
        Err(HarnessError::Quit) => {}
        other => panic!("{other:?}"),
    }
    assert!(matches!(harness.act(&[]), Err(HarnessError::Quit)));
    assert!(matches!(
        harness.wait_run(Duration::from_secs(1)),
        Err(HarnessError::Quit)
    ));

    harness.open(&path, false).unwrap();
    let report = harness.act(&[click(node(BRIGHTNESS))]).unwrap();
    assert_eq!(report.selection, [BRIGHTNESS]);
}
