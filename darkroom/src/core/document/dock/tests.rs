use super::*;
use scenarium::NodeId;

fn viewer(n: u128) -> TabRef {
    TabRef::ImageViewer(NodeId::from_u128(n))
}

fn main_tab() -> TabRef {
    TabRef::Graph
}

/// A refused close leaves the layout alone: the graph pane is never
/// closable, and a tab that isn't open anywhere resolves to nothing.
#[test]
fn close_is_dropped_for_the_graph_pane_or_a_tab_that_is_not_open() {
    let mut layout = seeded();
    let before = layout.clone();

    layout.apply(DockOp::CloseTab { tab: main_tab() });
    layout.apply(DockOp::CloseTab { tab: viewer(99) });

    assert_eq!(layout, before, "neither op removed a tab");
}

/// The invariant the dock's whole click path rests on. An op is built
/// from one frame's chip response and applied a phase later, with the
/// strip able to rearrange in between. Because ops name their *tab*
/// rather than its slot, the rearrangement can't redirect one onto
/// whatever slid into that slot.
#[test]
fn tab_ops_follow_their_tab_across_a_layout_change() {
    let mut layout = seeded();
    let primary = layout.primary().id;
    assert_eq!(
        layout.group(primary).unwrap().tabs,
        [main_tab(), TabRef::Preferences, viewer(1)]
    );

    // Built while the viewer sits at slot 2, applied after `Preferences`
    // left and the viewer slid down to slot 1.
    let close_viewer = DockOp::CloseTab { tab: viewer(1) };
    layout.apply(DockOp::CloseTab {
        tab: TabRef::Preferences,
    });
    assert_eq!(
        layout.group(primary).unwrap().tabs,
        [main_tab(), viewer(1)],
        "the viewer moved to slot 1"
    );

    layout.apply(close_viewer);
    assert_eq!(
        layout.group(primary).unwrap().tabs,
        [main_tab()],
        "the op closed the viewer, not whatever now occupies slot 2"
    );
}

/// Default layout + `Preferences` and one viewer tab in the primary
/// group.
fn seeded() -> DockLayout {
    let mut l = DockLayout::default();
    l.insert_tab(l.primary().id, TabRef::Preferences);
    l.insert_tab(l.primary().id, viewer(1));
    l
}

/// The root as a split, with its children resolved — for structural
/// asserts.
fn root_split(l: &DockLayout) -> (&DockSplit, &DockNode, &DockNode) {
    let DockNode::Split(s) = l.node(DockLayout::ROOT) else {
        panic!("root is a split");
    };
    (s, l.node(s.first), l.node(s.second))
}

#[test]
fn default_is_a_single_main_group() {
    let l = DockLayout::default();
    l.validate().unwrap();
    assert_eq!(l.groups().count(), 1);
    assert_eq!(l.primary().tabs, [main_tab()]);
    assert_eq!(l.focused, l.primary().id);
    assert_eq!(l.all_tabs().collect::<Vec<_>>(), [main_tab()]);
}

/// Pointer-driven focus moves `focused` and nothing else — no pane
/// switches tabs.
#[test]
fn focus_moves_only_the_focused_group() {
    let mut l = seeded();
    let primary = l.primary().id;
    l.move_tab(
        viewer(1),
        DockDrop::Split {
            group: primary,
            side: SplitSide::Right,
        },
    );
    let split_off = l.focused;
    assert_ne!(split_off, primary, "the new pane took focus");
    let actives: Vec<TabRef> = l.groups().map(TabGroup::active_tab).collect();

    l.focus(primary);
    l.validate().unwrap();
    assert_eq!(l.focused, primary, "focus moved to the pressed pane");
    assert_eq!(
        l.groups().map(TabGroup::active_tab).collect::<Vec<_>>(),
        actives,
        "focus alone moved — no pane switched tabs"
    );

    l.focus(primary);
    assert_eq!(l.focused, primary, "re-focusing the same pane is inert");
    l.validate().unwrap();
}

/// Focusing a group that isn't in the tree is a caller bug, not a stale
/// address to absorb: it would strand `focused` and fail validation at
/// the next save, long after the code that fabricated it.
#[test]
#[should_panic(expected = "is not in the tree")]
fn focusing_a_group_outside_the_tree_panics() {
    DockLayout::default().focus(TabGroupId::unique());
}

#[test]
fn split_move_and_collapse_roundtrip() {
    let mut l = seeded();
    let primary = l.primary().id;

    // Split the viewer off to the right: a Row split, primary first,
    // the new single-tab group second, focused. The re-packed vec is
    // pre-order: [split, primary, new] — validation pins that shape.
    l.move_tab(
        viewer(1),
        DockDrop::Split {
            group: primary,
            side: SplitSide::Right,
        },
    );
    l.validate().unwrap();
    let (s, first, second) = root_split(&l);
    assert_eq!(s.dir, SplitDir::Row);
    assert_eq!(s.ratio, 0.5);
    let DockNode::Group(first) = first else {
        panic!("primary stays first for a Right split");
    };
    assert_eq!(first.id, primary);
    assert_eq!(first.tabs, [main_tab(), TabRef::Preferences]);
    let DockNode::Group(second) = second else {
        panic!("new pane is a group");
    };
    assert_eq!(second.tabs, [viewer(1)]);
    assert_eq!(l.focused, second.id, "the new pane takes focus");

    // Moving the tab back into the primary strip collapses the split.
    l.move_tab(
        viewer(1),
        DockDrop::Into {
            group: primary,
            index: 1,
        },
    );
    l.validate().unwrap();
    assert!(
        matches!(l.node(DockLayout::ROOT), DockNode::Group(_)),
        "split collapsed"
    );
    assert_eq!(
        l.primary().tabs,
        [main_tab(), viewer(1), TabRef::Preferences],
        "tab re-inserted at the requested index"
    );
    assert_eq!(l.primary().active, 1, "moved tab becomes active");
    assert_eq!(l.focused, primary, "focus follows the destination");
}

#[test]
fn left_and_top_splits_put_the_new_pane_first() {
    for (side, dir) in [
        (SplitSide::Left, SplitDir::Row),
        (SplitSide::Top, SplitDir::Column),
    ] {
        let mut l = seeded();
        let primary = l.primary().id;
        l.move_tab(
            viewer(1),
            DockDrop::Split {
                group: primary,
                side,
            },
        );
        l.validate().unwrap();
        let (s, first, _) = root_split(&l);
        assert_eq!(s.dir, dir);
        let DockNode::Group(first) = first else {
            panic!("first child is the new pane");
        };
        assert_eq!(first.tabs, [viewer(1)], "{side:?} puts the new pane first");
    }
}

#[test]
fn degenerate_and_forbidden_moves_change_nothing() {
    let mut l = seeded();
    let primary = l.primary().id;
    l.move_tab(
        viewer(1),
        DockDrop::Split {
            group: primary,
            side: SplitSide::Right,
        },
    );
    let lone = l.focused;
    let before = l.clone();

    // A lone tab split off its own group re-creates the same shape.
    l.move_tab(
        viewer(1),
        DockDrop::Split {
            group: lone,
            side: SplitSide::Bottom,
        },
    );
    assert_eq!(l, before, "lone-tab self-split is a no-op");

    // A vanished target group is a no-op, not a panic.
    l.move_tab(
        TabRef::Preferences,
        DockDrop::Into {
            group: TabGroupId::unique(),
            index: 0,
        },
    );
    assert_eq!(l, before, "unknown drop target is ignored");
}

#[test]
fn closing_the_last_tab_collapses_and_refocuses() {
    let mut l = seeded();
    let primary = l.primary().id;
    l.move_tab(
        viewer(1),
        DockDrop::Split {
            group: primary,
            side: SplitSide::Bottom,
        },
    );
    l.close_tab(viewer(1));
    l.validate().unwrap();
    assert!(
        matches!(l.node(DockLayout::ROOT), DockNode::Group(_)),
        "empty pane collapsed"
    );
    assert_eq!(l.focused, primary, "dangling focus falls back to primary");

    // Main never closes, and a tab that isn't open anywhere no-ops.
    l.close_tab(main_tab());
    assert_eq!(l.primary().tabs[0], main_tab());
    l.close_tab(viewer(9));
    l.validate().unwrap();
}

#[test]
fn close_keeps_active_on_the_surviving_tab() {
    let mut l = seeded();
    l.activate(viewer(1));
    assert_eq!(l.group(l.focused).unwrap().active_tab(), viewer(1));

    // Closing the active last tab clamps active onto the previous one.
    l.close_tab(viewer(1));
    l.validate().unwrap();
    assert_eq!(l.primary().tabs, [main_tab(), TabRef::Preferences]);
    assert_eq!(l.primary().active, 1);

    // Closing a tab left of the active one shifts what `active`
    // points at — the old flat strip recomputed the index; per-group
    // clamping keeps it in range (pointing at the same slot is
    // accepted drift, the snapshot undo restores exact state).
    let mut l = seeded();
    l.activate(viewer(1));
    l.close_tab(TabRef::Preferences);
    l.validate().unwrap();
    assert_eq!(l.primary().active, 1, "clamped into range");
}

#[test]
fn same_group_reorder_uses_pre_move_indices() {
    // Strip [main, prefs, v1, v2]; every index below is a slot in
    // *that* strip, the way drop-zone math over the visible chips
    // computes it.
    let reordered = |from_tab: TabRef, index: usize| {
        let mut l = seeded();
        l.insert_tab(l.primary().id, viewer(2));
        l.move_tab(
            from_tab,
            DockDrop::Into {
                group: l.primary().id,
                index,
            },
        );
        l.validate().unwrap();
        l.primary().tabs.clone()
    };

    // Rightward: "prefs before v2" (slot 3) must not overshoot to
    // the end just because prefs' own removal shifted v2 left.
    assert_eq!(
        reordered(TabRef::Preferences, 3),
        [main_tab(), viewer(1), TabRef::Preferences, viewer(2)]
    );
    // Leftward needs no compensation: "v2 before prefs" (slot 1).
    assert_eq!(
        reordered(viewer(2), 1),
        [main_tab(), viewer(2), TabRef::Preferences, viewer(1)]
    );
    // Past-the-end clamps to an append.
    assert_eq!(
        reordered(TabRef::Preferences, 99),
        [main_tab(), viewer(1), viewer(2), TabRef::Preferences]
    );
}

#[test]
fn dock_path_packs_distinct_addresses() {
    // Sibling and cross-depth addresses never alias: the sentinel
    // bit keeps `[first]` (0b10), `[second]` (0b11), `[first,first]`
    // (0b100) and the root (0b1) all distinct.
    let paths = [
        DockPath::ROOT,
        DockPath::ROOT.first(),
        DockPath::ROOT.second(),
        DockPath::ROOT.first().first(),
        DockPath::ROOT.first().second(),
        DockPath::ROOT.second().first(),
        DockPath::ROOT.second().second(),
    ];
    for (i, a) in paths.iter().enumerate() {
        for b in &paths[i + 1..] {
            assert_ne!(a, b, "packed paths must not alias");
        }
    }
    // Turns replay in root→leaf order.
    assert_eq!(
        DockPath::ROOT
            .second()
            .first()
            .directions()
            .collect::<Vec<_>>(),
        [true, false]
    );
    assert_eq!(DockPath::ROOT.directions().count(), 0);
    assert_eq!(DockPath::default(), DockPath::ROOT);
}

#[test]
fn set_ratio_clamps_and_survives_stale_paths() {
    let mut l = seeded();
    let primary = l.primary().id;
    l.move_tab(
        viewer(1),
        DockDrop::Split {
            group: primary,
            side: SplitSide::Right,
        },
    );

    l.set_ratio(DockPath::ROOT, 0.7);
    let (s, ..) = root_split(&l);
    assert_eq!(s.ratio, 0.7);

    l.set_ratio(DockPath::ROOT, 0.01);
    let (s, ..) = root_split(&l);
    assert_eq!(s.ratio, RATIO_MIN, "ratio clamps to the floor");

    // Paths landing on a group, or walking past a leaf (stale after
    // a collapse), are ignored.
    l.set_ratio(DockPath::ROOT.first(), 0.5);
    l.set_ratio(DockPath::ROOT.first().second(), 0.5);
    l.validate().unwrap();
    let (s, ..) = root_split(&l);
    assert_eq!(s.ratio, RATIO_MIN, "stale paths change nothing");
}

#[test]
fn split_depth_is_capped_without_losing_the_tab() {
    let mut l = seeded();
    let primary = l.primary().id;
    for n in 2..=5 {
        l.insert_tab(primary, viewer(n));
    }

    // Chain splits off the freshly-focused group: each nests one
    // level deeper (1..=MAX_SPLIT_DEPTH).
    let mut target = primary;
    for n in 1..=4 {
        l.move_tab(
            viewer(n),
            DockDrop::Split {
                group: target,
                side: SplitSide::Right,
            },
        );
        target = l.focused;
    }
    l.validate().unwrap();
    assert_eq!(l.groups().count(), 5);
    assert_eq!(l.group_depth(target), Some(MAX_SPLIT_DEPTH));

    // The fifth split would nest past the cap: refused outright —
    // the layout is untouched and the tab stays where it was.
    let before = l.clone();
    l.move_tab(
        viewer(5),
        DockDrop::Split {
            group: target,
            side: SplitSide::Bottom,
        },
    );
    assert_eq!(l, before, "over-deep split is a no-op");
    assert!(
        l.primary().tabs.contains(&viewer(5)),
        "the refused split must not lose the tab"
    );
}

#[test]
fn retain_prunes_across_groups_and_collapses() {
    let mut l = seeded();
    let primary = l.primary().id;
    l.insert_tab(primary, viewer(2));
    l.move_tab(
        viewer(2),
        DockDrop::Split {
            group: primary,
            side: SplitSide::Right,
        },
    );

    // Pruning the split-off viewer collapses its pane; the primary
    // keeps the rest.
    l.retain_tabs(|t| t != viewer(2));
    l.validate().unwrap();
    assert!(matches!(l.node(DockLayout::ROOT), DockNode::Group(_)));
    assert_eq!(
        l.all_tabs().collect::<Vec<_>>(),
        [main_tab(), TabRef::Preferences, viewer(1)]
    );
}

#[test]
fn nested_splits_stay_canonical() {
    // Two nested splits: [rootsplit, primary, innersplit, v1, v2] in
    // pre-order once packed — validation's walk pins the exact shape,
    // and moving a tab out of the inner split dissolves only that
    // level.
    let mut l = seeded();
    let primary = l.primary().id;
    l.insert_tab(primary, viewer(2));
    l.move_tab(
        viewer(1),
        DockDrop::Split {
            group: primary,
            side: SplitSide::Right,
        },
    );
    let right = l.focused;
    l.move_tab(
        viewer(2),
        DockDrop::Split {
            group: right,
            side: SplitSide::Bottom,
        },
    );
    l.validate().unwrap();
    assert_eq!(l.groups().count(), 3);
    assert_eq!(
        l.all_tabs().collect::<Vec<_>>(),
        [main_tab(), TabRef::Preferences, viewer(1), viewer(2)],
        "pane order is left-to-right, top-to-bottom"
    );

    // Collapse the inner split; the outer one survives.
    l.move_tab(
        viewer(2),
        DockDrop::Into {
            group: primary,
            index: 99, // clamps to the end
        },
    );
    l.validate().unwrap();
    assert_eq!(l.groups().count(), 2);
    let (_, _, second) = root_split(&l);
    let DockNode::Group(second) = second else {
        panic!("inner split dissolved into the surviving group");
    };
    assert_eq!(second.tabs, [viewer(1)]);
    assert_eq!(
        l.primary().tabs,
        [main_tab(), TabRef::Preferences, viewer(2)],
        "the clamped index appended the tab"
    );
}

#[test]
fn serde_roundtrips_through_json() {
    let mut l = seeded();
    l.move_tab(
        viewer(1),
        DockDrop::Split {
            group: l.primary().id,
            side: SplitSide::Bottom,
        },
    );
    let bytes = common::serialize(&l, common::SerdeFormat::Json).unwrap();
    let back: DockLayout = common::deserialize(&bytes, common::SerdeFormat::Json).unwrap();
    assert_eq!(back, l);
}

#[test]
fn validate_rejects_each_corruption() {
    // Base: a valid two-pane layout — [split, primary(Main, Prefs),
    // viewer-pane(viewer 1)] — corrupted one invariant at a time via
    // direct field access (no public op can produce these states).
    let base = {
        let mut l = seeded();
        l.move_tab(
            viewer(1),
            DockDrop::Split {
                group: l.primary().id,
                side: SplitSide::Right,
            },
        );
        l.validate().unwrap();
        l
    };

    type Corrupt = fn(&mut DockLayout);
    let cases: [(&str, Corrupt, &str); 8] = [
        (
            "duplicate group id",
            |l| {
                let pid = l.primary().id;
                for n in &mut l.nodes {
                    if let DockNode::Group(g) = n {
                        g.id = pid;
                    }
                }
            },
            "appears twice",
        ),
        (
            "dangling focused",
            |l| l.focused = TabGroupId::unique(),
            "focused group",
        ),
        (
            "active out of range",
            |l| {
                let DockNode::Group(g) = &mut l.nodes[1] else {
                    panic!("slot 1 is the primary group");
                };
                g.active = g.tabs.len();
            },
            "active tab out of range",
        ),
        (
            "ratio out of bounds",
            |l| {
                let DockNode::Split(s) = &mut l.nodes[0] else {
                    panic!("root is a split");
                };
                s.ratio = 0.95;
            },
            "split ratio",
        ),
        (
            "children out of pre-order",
            |l| {
                let DockNode::Split(s) = &mut l.nodes[0] else {
                    panic!("root is a split");
                };
                std::mem::swap(&mut s.first, &mut s.second);
            },
            "canonical pre-order",
        ),
        (
            "child index past the end",
            |l| {
                l.nodes.truncate(2);
            },
            "dock node index",
        ),
        (
            "unreachable trailing slot",
            |l| {
                let DockNode::Split(s) = &mut l.nodes[0] else {
                    panic!("root is a split");
                };
                // Root points only at slot 1; slots 2.. are orphaned.
                *s = DockSplit {
                    first: NodeIdx(1),
                    second: NodeIdx(1),
                    ..*s
                };
            },
            "canonical pre-order",
        ),
        (
            "empty group",
            |l| {
                let DockNode::Group(g) = &mut l.nodes[2] else {
                    panic!("slot 2 is the split-off viewer pane");
                };
                g.tabs.clear();
            },
            "is empty",
        ),
    ];
    for (name, corrupt, expected) in cases {
        let mut l = base.clone();
        corrupt(&mut l);
        let err = l.validate().unwrap_err().to_string();
        assert!(err.contains(expected), "{name}: unexpected error: {err}");
    }

    // A layout with no Main tab anywhere is refused too — dropping it
    // from the primary group leaves both groups non-empty, so nothing
    // else trips first.
    let mut l = base.clone();
    let DockNode::Group(g) = &mut l.nodes[1] else {
        panic!("slot 1 is the primary group");
    };
    g.tabs.retain(|t| *t != main_tab());
    g.active = 0;
    let err = l.validate().unwrap_err().to_string();
    assert!(
        err.contains("no group holds the Main graph tab"),
        "missing Main: unexpected error: {err}"
    );
}
