use glam::Vec2;
use palantir::{Modifiers, PointerButton};
use scenarium::{InputPort, OutputPort};

use crate::core::document::internals::DocumentFixture;
use crate::core::document::item_id::ItemId;
use crate::core::document::tie::TieId;
use crate::core::document::wire_key::WireKey;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::route_insert::RouteInsert;
use crate::gui::pane::graph_ui::gesture::tie_ui::tie_widget::{AngleSnap, knob_angle};
use crate::gui::pane::graph_ui::internals::CanvasHarness;
use crate::gui::port_ref::PortRef;

/// Modifiers with Ctrl and Shift as given.
fn held(ctrl: bool, shift: bool) -> Modifiers {
    Modifiers {
        ctrl,
        shift,
        ..Modifiers::default()
    }
}

/// A drag on bare canvas latches the gesture its chord names: RMB-drag and Ctrl+LMB-drag the
/// breaker, Shift+RMB-drag and Ctrl+Shift+LMB-drag the tie line, and Shift+LMB-drag still the
/// band selection, which Shift extends.
#[test]
fn the_tie_line_is_the_breakers_chord_plus_shift() {
    #[derive(Debug, PartialEq)]
    enum Latched {
        Breaker,
        TieLine,
        Band,
    }
    let cases = [
        (PointerButton::Right, held(false, false), Latched::Breaker),
        (PointerButton::Right, held(false, true), Latched::TieLine),
        (PointerButton::Left, held(true, false), Latched::Breaker),
        (PointerButton::Left, held(true, true), Latched::TieLine),
        (PointerButton::Left, held(false, true), Latched::Band),
    ];
    for (button, modifiers, want) in cases {
        let mut h = CanvasHarness::new(DocumentFixture::default());
        h.prime(2);
        h.ui.set_modifiers(modifiers);
        h.ui.press_button_at(button, Vec2::new(300.0, 300.0));
        h.frame();
        h.ui.drag_to(Vec2::new(340.0, 380.0));
        h.frame();
        let graph_ui = &h.graph_ui;
        let latched = [
            (graph_ui.breaker_ui.in_flight(), Latched::Breaker),
            (graph_ui.tie_ui.in_flight(), Latched::TieLine),
            (graph_ui.selection_ui.in_flight(), Latched::Band),
        ]
        .into_iter()
        .filter_map(|(on, which)| on.then_some(which))
        .collect::<Vec<_>>();
        assert_eq!(latched, [want], "{button:?} with {modifiers:?}");
    }
}

/// Two plain nodes on the left whose outputs feed two plain nodes on the right, each output to
/// two inputs: four wires between x 240 and 700, which a line down x 470 crosses.
struct Crossed {
    h: CanvasHarness,
    wires: [WireKey; 4],
    /// Where the line runs, between the outputs and the inputs.
    x: f32,
}

fn crossed(tie_first: bool) -> Crossed {
    let mut fixture = DocumentFixture::default();
    let [left_a, left_b] = [40.0, 300.0].map(|y| fixture.node_at(Vec2::new(40.0, y)));
    let [right_a, right_b] = [40.0, 300.0].map(|y| fixture.node_at(Vec2::new(700.0, y)));
    let wires = [
        (left_a, right_a, 0),
        (left_a, right_b, 1),
        (left_b, right_a, 1),
        (left_b, right_b, 0),
    ]
    .map(|(from, to, input)| fixture.wire(OutputPort::new(from, 0), InputPort::new(to, input)));
    if tie_first {
        fixture.tie(Vec2::ZERO, [wires[0]]);
    }
    let mut h = CanvasHarness::new(fixture);
    h.prime(3);
    let output = h.port_center(PortRef::from(OutputPort::new(left_a, 0)));
    let input = h.port_center(PortRef::from(InputPort::new(right_a, 0)));
    let x = f32::midpoint(output.x, input.x);
    Crossed { h, wires, x }
}

/// Draw a Shift+RMB tie line from `from` to `to` and release it, returning the release
/// frame's intents. The line latches where the drag first moves, so the first step is a short
/// one toward `to`.
fn tie_line(h: &mut CanvasHarness, from: Vec2, to: Vec2) -> Vec<GraphIntent> {
    h.ui.set_modifiers(held(false, true));
    h.ui.press_button_at(PointerButton::Right, from);
    h.frame();
    h.ui.drag_to(from + (to - from).normalize() * 6.0);
    h.frame();
    h.ui.drag_to(to);
    h.frame();
    h.frame();
    h.ui.release_button(PointerButton::Right);
    h.frame()
}

/// A line down `x` across all four wires ties them into one new tie, each at its only piece,
/// placed at the mean of the crossings, which all lie on the line, and selects it.
#[test]
fn a_line_across_wires_ties_them() {
    let Crossed { mut h, wires, x } = crossed(false);
    let intents = tie_line(&mut h, Vec2::new(x, 5.0), Vec2::new(x, 790.0));
    let [
        GraphIntent::TieWires {
            tie,
            pos,
            wires: tied,
        },
        GraphIntent::SetSelection { to },
    ] = &*intents
    else {
        panic!("a tie line ties and selects: {intents:?}");
    };
    assert_eq!(pos.x, x, "the crossings lie on the line");
    let mut tied: Vec<RouteInsert> = tied.clone();
    tied.sort_by_key(|insert| insert.wire);
    let mut want = wires.map(|wire| RouteInsert { wire, at: 0 });
    want.sort_by_key(|insert| insert.wire);
    assert_eq!(tied, want);
    assert_eq!(*to, [ItemId::Tie(*tie)].into_iter().collect());
}

/// A line across a tie and wires joins the others to that tie, leaving out the wire it holds,
/// and selects it. The line slants, as a band across level strands stands upright.
#[test]
fn a_line_across_a_tie_joins_it() {
    let Crossed { mut h, wires, x } = crossed(true);
    let tie = *h.document().main_view.ties.keys().next().unwrap();
    // On the line, between the wires.
    let centre = Vec2::new(x, 250.0);
    h.document_mut().main_view.ties.get_mut(&tie).unwrap().pos = centre;
    h.frame();
    let intents = tie_line(
        &mut h,
        centre - Vec2::new(15.0, 150.0),
        centre + Vec2::new(15.0, 150.0),
    );
    let [
        GraphIntent::JoinTie {
            tie: joined,
            wires: added,
        },
        GraphIntent::SetSelection { to },
    ] = &*intents
    else {
        panic!("a line across a tie joins it: {intents:?}");
    };
    assert_eq!(*joined, tie);
    assert!(
        added
            .iter()
            .all(|insert| insert.wire != wires[0] && insert.at == 0),
        "the tie's own wire is left out: {added:?}"
    );
    assert_eq!(added.len(), 3, "the three other wires join: {added:?}");
    assert_eq!(*to, [ItemId::Tie(tie)].into_iter().collect());
}

/// A line across nothing ties nothing.
#[test]
fn a_line_across_nothing_ties_nothing() {
    let Crossed { mut h, .. } = crossed(false);
    let intents = tie_line(&mut h, Vec2::new(30.0, 760.0), Vec2::new(200.0, 790.0));
    assert!(intents.is_empty(), "{intents:?}");
}

/// The knob sets the pointer's angle about the centre, from `+x` toward `+y`: at 52° it sets
/// 52°, and with Shift the nearest 15° step, 45°. Straight up is -90°, which wraps to 270°.
/// 52° survives `cos`, `sin` and `atan2` to within `1e-4`, the rounding of three `f32` steps.
#[test]
fn the_knob_turns_to_the_pointer_in_free_or_fifteen_degree_steps() {
    let (sin, cos) = 52f32.to_radians().sin_cos();
    let at_52 = Vec2::new(cos, sin) * 40.0;
    assert!((knob_angle(at_52, AngleSnap::Free) - 52.0).abs() < 1e-4);
    assert_eq!(knob_angle(at_52, AngleSnap::Steps), 45.0);
    assert_eq!(knob_angle(Vec2::new(0.0, -10.0), AngleSnap::Free), 270.0);
    assert_eq!(knob_angle(Vec2::new(10.0, -1e-9), AngleSnap::Steps), 0.0);
}

/// One tie at (470, 250) on a wire between two plain nodes, selected or not.
fn one_tie(selected: bool) -> (CanvasHarness, TieId) {
    let mut fixture = DocumentFixture::default();
    let from = fixture.node_at(Vec2::new(40.0, 200.0));
    let to = fixture.node_at(Vec2::new(700.0, 200.0));
    let wire = fixture.wire(OutputPort::new(from, 0), InputPort::new(to, 0));
    let tie = fixture.tie(Vec2::new(470.0, 250.0), [wire]);
    if selected {
        fixture.document.main_view.selected.insert(ItemId::Tie(tie));
    }
    let mut h = CanvasHarness::new(fixture);
    h.prime(3);
    (h, tie)
}

/// A click on a tie selects it alone, as a click on a node does; a drag of it moves it, alone,
/// by the pointer's travel; and a band over its centre sweeps it.
#[test]
fn a_tie_clicks_drags_and_sweeps_as_a_node_does() {
    let (mut h, tie) = one_tie(false);
    let centre = Vec2::new(470.0, 250.0);
    h.ui.click_at(centre);
    let clicked = h.frame();
    assert!(
        matches!(&*clicked, [GraphIntent::SetSelection { to }] if *to == [ItemId::Tie(tie)].into_iter().collect()),
        "{clicked:?}"
    );

    let (mut h, tie) = one_tie(false);
    h.ui.press_at(centre);
    h.frame();
    h.ui.drag_to(centre + Vec2::new(10.0, 0.0));
    h.frame();
    h.ui.drag_to(centre + Vec2::new(30.0, -20.0));
    let moved = h.frame();
    let moves: Vec<_> = moved
        .iter()
        .filter_map(|intent| match intent {
            GraphIntent::MoveSelection {
                members, offset, ..
            } => Some((members.to_vec(), *offset)),
            _ => None,
        })
        .collect();
    assert_eq!(
        moves
            .last()
            .map(|(members, _)| members.iter().map(|m| m.item).collect::<Vec<_>>()),
        Some(vec![ItemId::Tie(tie)]),
        "the tie drags alone: {moved:?}"
    );

    let (mut h, tie) = one_tie(false);
    h.ui.press_at(Vec2::new(430.0, 600.0));
    h.frame();
    h.ui.drag_to(Vec2::new(430.0, 590.0));
    h.frame();
    h.ui.drag_to(Vec2::new(520.0, 230.0));
    h.frame();
    h.ui.release_button(PointerButton::Left);
    let swept = h.frame();
    assert!(
        matches!(&*swept, [GraphIntent::SetSelection { to }] if *to == [ItemId::Tie(tie)].into_iter().collect()),
        "{swept:?}"
    );
}

/// A selected tie's knob turns the band to the pointer, each frame of the drag naming one undo
/// gesture, and a double-click frees the angle.
#[test]
fn the_knob_of_a_selected_tie_pins_and_frees_its_angle() {
    use crate::core::document::tie::TieAngle;
    use crate::gui::pane::graph_ui::gesture::tie_ui::tie_widget::knob_wid;

    let (mut h, tie) = one_tie(true);
    let knob = h.ui.center_of(knob_wid(tie));
    h.ui.press_at(knob);
    h.frame();
    h.ui.drag_to(knob + Vec2::new(0.0, 10.0));
    h.frame();
    let below = Vec2::new(470.0, 250.0) + Vec2::new(0.0, 60.0);
    h.ui.drag_to(below);
    let turned = h.frame();
    let turns: Vec<_> = turned
        .iter()
        .filter_map(|intent| match intent {
            GraphIntent::SetTieAngle {
                tie: turned,
                angle,
                gesture,
            } if *turned == tie => Some((*angle, gesture.is_some())),
            _ => None,
        })
        .collect();
    assert_eq!(
        turns,
        [(TieAngle::Pinned(90.0), true)],
        "straight below the centre is 90°: {turned:?}"
    );
    h.ui.release_button(PointerButton::Left);
    h.frame();

    let (mut h, tie) = one_tie(true);
    h.document_mut().main_view.ties.get_mut(&tie).unwrap().angle = TieAngle::Pinned(30.0);
    h.prime(2);
    let knob = h.ui.center_of(knob_wid(tie));
    h.advance_past_double_click();
    h.ui.click_at(knob);
    h.frame();
    h.ui.click_at(knob);
    let freed = h.frame();
    assert!(
        freed.iter().any(|intent| matches!(
            intent,
            GraphIntent::SetTieAngle {
                angle: TieAngle::Auto,
                gesture: None,
                ..
            }
        )),
        "{freed:?}"
    );
}

/// A tie at (470, 250) on two wires from one plain node to two inputs of another.
fn two_wire_tie() -> (CanvasHarness, TieId, [WireKey; 2]) {
    let mut fixture = DocumentFixture::default();
    let from = fixture.node_at(Vec2::new(40.0, 200.0));
    let to = fixture.node_at(Vec2::new(700.0, 200.0));
    let wires =
        [0, 1].map(|input| fixture.wire(OutputPort::new(from, 0), InputPort::new(to, input)));
    let tie = fixture.tie(Vec2::new(470.0, 250.0), wires);
    let mut h = CanvasHarness::sized(fixture, glam::UVec2::new(1200, 900));
    h.prime(3);
    (h, tie, wires)
}

/// A right-click on a tie selects it and opens its menu: a Release entry for each wire, in
/// wire order, then Pin angle, or Free angle on a pinned tie, and Remove tie. Hovering a Release
/// entry lights its strand, and each entry raises its edit: the release of its wire, a pin at the
/// angle the band stands at now, a free, and the tie's removal.
#[test]
fn the_tie_menu_releases_pins_and_removes() {
    use crate::core::document::tie::TieAngle;
    use crate::gui::pane::graph_ui::gesture::tie_ui::tie_widget::{AngleSnap, knob_angle};

    let centre = Vec2::new(470.0, 250.0);
    let open = |h: &mut CanvasHarness| {
        h.ui.right_click_at(centre);
        let opened = h.frame();
        h.prime(2);
        (opened, h.menu_rows(centre))
    };

    let (mut h, tie, mut wires) = two_wire_tie();
    wires.sort_unstable();
    let (opened, rows) = open(&mut h);
    assert!(
        matches!(&*opened, [GraphIntent::SetSelection { to }] if *to == [ItemId::Tie(tie)].into_iter().collect()),
        "the right-click selects the tie: {opened:?}"
    );
    assert_eq!(rows.len(), 4, "two releases, a pin and a removal");
    // Each entry names its rows by text: the wire from the producer's output to the consumer's
    // input, through the plain func's port names.
    let mut listed = Vec::new();
    h.ui.ui().inspect(&mut listed);
    let text = |id| {
        listed
            .iter()
            .find(|widget| widget.id == id)
            .map(|widget| widget.text.clone())
            .unwrap()
    };
    let names: Vec<String> = h
        .document()
        .graph
        .iter()
        .map(|node| node.name.clone())
        .collect();
    assert_eq!(names[0], names[1], "both plain nodes share the func's name");
    let name = &names[0];
    assert_eq!(
        [rows[0], rows[1], rows[2], rows[3]].map(text),
        [
            format!("Release {name}.out → {name}.a"),
            format!("Release {name}.out → {name}.b"),
            "Pin angle".to_string(),
            "Remove tie".to_string(),
        ]
    );
    h.ui.move_to(h.ui.center_of(rows[1]));
    h.frame();
    assert_eq!(
        h.graph_ui.tie_menu.lit(),
        Some(wires[1]),
        "the hovered entry lights its wire"
    );

    for row in [0, 1, 3] {
        let (mut h, tie, mut wires) = two_wire_tie();
        wires.sort_unstable();
        let want = match row {
            3 => GraphIntent::RemoveTie { tie },
            release => GraphIntent::ReleaseWire {
                tie,
                wire: wires[release],
            },
        };
        let (_, rows) = open(&mut h);
        h.ui.click_on(rows[row]);
        let picked = h.frame();
        assert_eq!(format!("{picked:?}"), format!("{:?}", [want]), "row {row}");
    }

    let (mut h, tie, _) = two_wire_tie();
    let (_, rows) = open(&mut h);
    let standing = knob_angle(h.graph_ui.ties.band(tie).unwrap().axis, AngleSnap::Free);
    h.ui.click_on(rows[2]);
    let pinned = h.frame();
    assert!(
        matches!(
            &*pinned,
            [GraphIntent::SetTieAngle { angle: TieAngle::Pinned(degrees), gesture: None, .. }]
                if *degrees == standing
        ),
        "a pin keeps the band where it stands, at {standing}°: {pinned:?}"
    );

    // On a pinned tie the same row frees the angle.
    let (mut h, tie, _) = two_wire_tie();
    h.document_mut().main_view.ties.get_mut(&tie).unwrap().angle = TieAngle::Pinned(30.0);
    h.prime(2);
    let (_, rows) = open(&mut h);
    h.ui.click_on(rows[2]);
    let freed = h.frame();
    assert!(
        matches!(
            &*freed,
            [GraphIntent::SetTieAngle {
                angle: TieAngle::Auto,
                gesture: None,
                ..
            }]
        ),
        "a pinned tie's menu frees its angle: {freed:?}"
    );
}
