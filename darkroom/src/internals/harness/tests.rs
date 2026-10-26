use std::path::PathBuf;

use common::{TempDir, fs};
use glam::{UVec2, Vec2};
use palantir::{Modifiers, PointerButton};
use scenarium::NodeId;

use crate::core::document::internals::DocumentFixture;
use crate::core::open_document::OpenDocument;
use crate::internals::harness::action::{Action, DRAG_STEPS};
use crate::internals::harness::error::HarnessError;
use crate::internals::harness::locator::{Locator, Side};
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

/// A project of one node of each func named, from `harness`'s library, saved in `dir`.
fn project(harness: &Harness, dir: &TempDir, funcs: &[&str]) -> PathBuf {
    let library = harness.app.library();
    let mut fixture = DocumentFixture::default();
    for name in funcs {
        fixture.add(
            library
                .by_name(name)
                .expect("the editor's library has the func"),
        );
    }
    let path = dir.join("scene.darkroom");
    OpenDocument::over(fixture.document).save_to(&path).unwrap();
    path
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
    let path = project(&harness, &dir, &[BRIGHTNESS, CONVERT]);
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
            .item_placements[&id]
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
    let path = project(&harness, &dir, &[BRIGHTNESS]);
    harness.open(&path, false).unwrap();
    let id = node_id(&harness, BRIGHTNESS);
    let pos = harness
        .app
        .open_document()
        .document
        .main_view
        .item_placements[&id]
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
    let path = project(&harness, &dir, &[BRIGHTNESS]);
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
    let path = project(&harness, &dir, &[BRIGHTNESS, BRIGHTNESS]);
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
    let path = project(&harness, &dir, &[BRIGHTNESS]);
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
    let path = project(&harness, &dir, &[BRIGHTNESS]);
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
/// button held; and the error an unanswered dialog logs during `act` reaches the next `state`.
#[test]
fn a_failed_action_leaves_nothing_held_and_the_log_reaches_state() {
    let mut harness = harness();
    let dir = TempDir::new("darkroom-harness-failed");
    let path = project(&harness, &dir, &[BRIGHTNESS]);
    harness.open(&path, false).unwrap();
    assert!(matches!(
        harness.act(&[drag(node(BRIGHTNESS), node("missing"))]),
        Err(HarnessError::NotFound { .. })
    ));
    let report = harness.act(&[click(node(BRIGHTNESS))]).unwrap();
    assert_eq!(report.selection, [BRIGHTNESS]);

    harness.act(&[Action::Key("ctrl+o".to_string())]).unwrap();
    let log = harness.state(Detail::Concise, None).log;
    assert!(
        log.iter()
            .any(|line| line.contains("no scripted answer was left")),
        "{log:?}"
    );
}

/// A link to a folder inside the project is refused: following it could copy the folder into
/// itself.
#[test]
fn a_linked_folder_is_refused() {
    let mut harness = harness();
    let dir = TempDir::new("darkroom-harness-link");
    let path = project(&harness, &dir, &[BRIGHTNESS]);
    fs::symlink(dir.path(), dir.join("loop")).unwrap();
    assert!(matches!(
        harness.open(&path, false),
        Err(HarnessError::LinkedFolder { .. })
    ));
}
