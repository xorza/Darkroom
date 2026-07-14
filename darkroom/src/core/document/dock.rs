//! The dock layout: a binary split tree whose leaves are tab groups.
//! Replaces the old flat tab strip — each [`TabGroup`] renders as one
//! pane with its own strip, and a [`DockSplit`] divides the space
//! between two child nodes at a draggable `ratio`. Pure data + pure
//! ops; every mutation is snapshot-diffed by the intent layer
//! (`DocStep::Dock { from, to }`), so ops apply in place and report
//! nothing.
//!
//! **Flat storage.** The tree lives in one `Vec<DockNode>` with
//! [`NodeIdx`] children — no per-node boxes. The vec is kept
//! *canonical*: pre-order from the root at slot 0, no dead slots
//! ([`DockLayout::normalize`] re-packs after every structural op). That
//! makes `Vec` equality structural equality (the undo layer's no-op
//! diff depends on it) and group iteration a plain vec scan in
//! left-to-right pane order.
//!
//! Invariants (checked by [`DockLayout::check`]):
//! - the vec is canonical pre-order, fully reachable from slot 0;
//! - exactly one group holds the `Main` graph tab (the *primary* group,
//!   successor of the old "tabs[0] is Main" rule);
//! - every graph tab lives in the primary group — the edit pipeline
//!   renders one canvas (see the docking plan; lifting this is the
//!   multi-canvas phase);
//! - no group is empty, no tab appears twice, group ids are unique,
//!   per-group `active` is in range, `focused` names a live group,
//!   ratios stay in `RATIO_MIN..=RATIO_MAX`.

use anyhow::{Context, Result, ensure};
use common::id_type;
use serde::{Deserialize, Serialize};

use crate::core::document::{GraphRef, TabRef};

id_type!(TabGroupId);

/// Split-ratio clamp: neither pane can be squeezed below a tenth of the
/// split, so a divider can't be dragged into an unrecoverable sliver.
const RATIO_MIN: f32 = 0.1;
const RATIO_MAX: f32 = 0.9;

/// Most nested splits allowed on any root-to-leaf chain (up to 16
/// panes) — [`DockLayout::move_tab`] refuses splits past it. Keeps the
/// UI sane and every split address comfortably inside a [`DockPath`].
const MAX_SPLIT_DEPTH: u32 = 4;

/// A split's address: the turns taken from the root, packed into one
/// byte — a leading sentinel bit, then one bit per level (`0` = first
/// child, `1` = second). The root split is the bare sentinel. One
/// `Copy` byte instead of a `Vec<bool>`, with capacity for 7 levels —
/// [`MAX_SPLIT_DEPTH`] keeps real trees well inside that.
///
/// Like any address into the layout it's only stable between
/// structural changes; a stale path that no longer lands on a split is
/// ignored by the ops it feeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DockPath(u8);

impl DockPath {
    /// The root node's address (the empty path).
    pub const ROOT: DockPath = DockPath(1);

    /// The address of `self`'s first (left/top) child.
    pub fn first(self) -> DockPath {
        self.child(false)
    }

    /// The address of `self`'s second (right/bottom) child.
    pub fn second(self) -> DockPath {
        self.child(true)
    }

    fn child(self, second: bool) -> DockPath {
        assert!(
            self.0 < 0x80,
            "dock path capacity (7 levels) exceeded — MAX_SPLIT_DEPTH should stop far earlier"
        );
        DockPath((self.0 << 1) | second as u8)
    }

    /// Turns from the root, in root→leaf order. Saturating so the
    /// invalid sentinel-less `0` byte (reachable only through serde)
    /// yields no turns instead of underflowing.
    fn directions(self) -> impl Iterator<Item = bool> {
        let depth = 7u32.saturating_sub(self.0.leading_zeros());
        (0..depth).rev().map(move |i| (self.0 >> i) & 1 == 1)
    }
}

impl Default for DockPath {
    fn default() -> Self {
        Self::ROOT
    }
}

/// Index of a node in [`DockLayout`]'s flat tree. Only stable between
/// structural changes (normalize re-packs); long-lived references use
/// [`TabGroupId`], and an op fed a stale index bounds-checks and no-ops.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeIdx(u32);

impl NodeIdx {
    fn usize(self) -> usize {
        self.0 as usize
    }
}

/// How a [`DockSplit`] arranges its children: `Row` side by side
/// (vertical divider), `Column` stacked (horizontal divider).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitDir {
    Row,
    Column,
}

/// Which edge of a pane a split lands on — the new pane takes that
/// edge's half. `Left`/`Right` split into a [`SplitDir::Row`],
/// `Top`/`Bottom` into a [`SplitDir::Column`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitSide {
    Left,
    Right,
    Top,
    Bottom,
}

impl SplitSide {
    fn dir(self) -> SplitDir {
        match self {
            SplitSide::Left | SplitSide::Right => SplitDir::Row,
            SplitSide::Top | SplitSide::Bottom => SplitDir::Column,
        }
    }

    /// Whether the new pane becomes the split's *first* child (left /
    /// top).
    fn new_pane_first(self) -> bool {
        matches!(self, SplitSide::Left | SplitSide::Top)
    }
}

/// Where a moved tab lands — the payload of [`DockOp::MoveTab`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DockDrop {
    /// Join `group`'s strip at `index` (clamped to its length).
    Into { group: TabGroupId, index: usize },
    /// Split `group`'s pane; the tab becomes a fresh single-tab group on
    /// the given side.
    Split { group: TabGroupId, side: SplitSide },
}

/// One dock-layout mutation, executed by [`DockLayout::apply`]. The
/// single op vocabulary the whole pipeline speaks: the dock UI
/// constructs one, `UiAction::Dock` transports it, `Intent::Dock`
/// records it as a before/after snapshot, and `apply` runs it. Ops fed
/// stale addresses (a group or split that no longer exists) no-op, and
/// the snapshot diff drops them.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum DockOp {
    /// Make `group`'s tab at `index` visible and focus the group.
    ActivateTab { group: TabGroupId, index: usize },
    /// Close `group`'s tab at `index`. The `Main` tab never closes —
    /// the op refuses it.
    CloseTab { group: TabGroupId, index: usize },
    /// Move `tab` to `to` — into another strip or splitting a pane.
    MoveTab { tab: TabRef, to: DockDrop },
    /// Set the ratio of the split at `split` (its packed root path).
    /// Emitted per frame by a divider drag; coalesces per split.
    SetRatio { split: DockPath, ratio: f32 },
}

/// One pane's tab strip: the open tabs plus which one is visible.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabGroup {
    pub id: TabGroupId,
    /// Non-empty; a group whose last tab closes collapses out of the tree.
    pub tabs: Vec<TabRef>,
    /// Index of the visible tab; always in range.
    pub active: usize,
}

impl TabGroup {
    pub fn active_tab(&self) -> TabRef {
        self.tabs[self.active]
    }

    /// Remove the tab at `index`, keeping `active` on a surviving slot.
    fn remove_tab(&mut self, index: usize) {
        self.tabs.remove(index);
        self.clamp_active();
    }

    fn clamp_active(&mut self) {
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
    }
}

/// One node of the flat tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DockNode {
    Split(DockSplit),
    Group(TabGroup),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DockSplit {
    pub dir: SplitDir,
    /// The first child's share of the free space, in
    /// `RATIO_MIN..=RATIO_MAX`.
    pub ratio: f32,
    pub first: NodeIdx,
    pub second: NodeIdx,
}

/// The whole pane arrangement: the flat split tree plus which group has
/// focus (keyboard-shortcut routing + where opened tabs land). Persisted
/// on the `Document` and snapshot into every dock undo step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DockLayout {
    /// Canonical pre-order (see the module doc). Private so every
    /// structural mutation goes through the ops that renormalize.
    nodes: Vec<DockNode>,
    pub focused: TabGroupId,
}

impl Default for DockLayout {
    /// A single group holding the `Main` graph. `nil` keys the default
    /// primary group deterministically (defaults compare equal); split
    /// offspring get `unique()` ids.
    fn default() -> Self {
        let primary = TabGroup {
            id: TabGroupId::nil(),
            tabs: vec![TabRef::Graph(GraphRef::Main)],
            active: 0,
        };
        Self {
            focused: primary.id,
            nodes: vec![DockNode::Group(primary)],
        }
    }
}

/// A tab's position in the tree: which group holds it and where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabAddress {
    pub group: TabGroupId,
    pub index: usize,
}

impl DockLayout {
    /// The root node's index — always slot 0 in the canonical order.
    pub const ROOT: NodeIdx = NodeIdx(0);

    /// The node at `idx` — the render walk follows [`DockSplit`]'s child
    /// indices through this.
    pub fn node(&self, idx: NodeIdx) -> &DockNode {
        &self.nodes[idx.usize()]
    }

    /// The leaf groups in left-to-right, top-to-bottom pane order — in
    /// canonical pre-order storage that's simply vec order.
    pub fn groups(&self) -> impl Iterator<Item = &TabGroup> {
        self.nodes.iter().filter_map(|n| match n {
            DockNode::Group(g) => Some(g),
            DockNode::Split(_) => None,
        })
    }

    /// Every open tab across every group, in [`Self::groups`] order.
    pub fn all_tabs(&self) -> impl Iterator<Item = TabRef> + '_ {
        self.groups().flat_map(|g| g.tabs.iter().copied())
    }

    pub fn group(&self, id: TabGroupId) -> Option<&TabGroup> {
        self.groups().find(|g| g.id == id)
    }

    pub fn group_mut(&mut self, id: TabGroupId) -> Option<&mut TabGroup> {
        self.nodes.iter_mut().find_map(|n| match n {
            DockNode::Group(g) if g.id == id => Some(g),
            _ => None,
        })
    }

    /// The group holding the `Main` graph tab — the one pane that hosts
    /// graph canvases.
    pub fn primary(&self) -> &TabGroup {
        self.groups()
            .find(|g| g.tabs.contains(&TabRef::Graph(GraphRef::Main)))
            .expect("a group holds the Main tab")
    }

    pub fn find_tab(&self, tab: TabRef) -> Option<TabAddress> {
        self.groups().find_map(|g| {
            g.tabs
                .iter()
                .position(|t| *t == tab)
                .map(|index| TabAddress { group: g.id, index })
        })
    }

    /// Execute one [`DockOp`] — the dispatch behind every recorded
    /// layout mutation.
    pub fn apply(&mut self, op: DockOp) {
        match op {
            DockOp::ActivateTab { group, index } => self.activate(group, index),
            DockOp::CloseTab { group, index } => self.close_tab(group, index),
            DockOp::MoveTab { tab, to } => self.move_tab(tab, to),
            DockOp::SetRatio { split, ratio } => self.set_ratio(split, ratio),
        }
    }

    /// Focus `group` and make its `index` tab visible. Out-of-range
    /// input is ignored.
    pub fn activate(&mut self, group: TabGroupId, index: usize) {
        if let Some(g) = self.group_mut(group)
            && index < g.tabs.len()
        {
            g.active = index;
            self.focused = group;
        }
    }

    /// `tab`'s address, appending it to `group`'s strip when it isn't
    /// open anywhere — the shared non-undoable half of opening any tab
    /// (callers focus it through a recorded activation). Unlike the
    /// intent-fed ops this is a direct call whose callers read a live
    /// group id in the same call chain, so a dead id is a logic error,
    /// not tolerable staleness.
    pub fn find_or_insert(&mut self, tab: TabRef, group: TabGroupId) -> TabAddress {
        match self.find_tab(tab) {
            Some(addr) => addr,
            None => self.insert_tab(group, tab),
        }
    }

    /// Raw append of `tab` to `group`'s strip; [`Self::find_or_insert`]
    /// owns the dedup.
    fn insert_tab(&mut self, group: TabGroupId, tab: TabRef) -> TabAddress {
        let g = self.group_mut(group).expect("insert target group exists");
        g.tabs.push(tab);
        let index = g.tabs.len() - 1;
        TabAddress { group, index }
    }

    /// Close `group`'s tab at `index`. The `Main` tab never closes (also
    /// guarded at intent build). A group emptied by the close collapses
    /// out of the tree; a vanished focus falls back to the primary group.
    pub fn close_tab(&mut self, group: TabGroupId, index: usize) {
        let Some(g) = self.group_mut(group) else {
            return;
        };
        match g.tabs.get(index) {
            None | Some(&TabRef::Graph(GraphRef::Main)) => return,
            Some(_) => g.remove_tab(index),
        }
        self.normalize();
    }

    /// Move `tab` to `drop`, collapsing whatever its departure empties.
    /// An `Into` index addresses the target strip *as the caller saw
    /// it* (pre-move) — a reorder within one group lands exactly where
    /// the drop-zone math over the visible chips said, despite the
    /// tab's own removal shifting the slots. Graph tabs are pinned to
    /// the primary group and never move. The destination group (fresh
    /// one for a split) takes the tab as its active and gains focus.
    /// Degenerate moves — a split off a group that holds only this tab,
    /// targeting itself — leave the layout unchanged (the snapshot diff
    /// drops them).
    pub fn move_tab(&mut self, tab: TabRef, drop: DockDrop) {
        if matches!(tab, TabRef::Graph(_)) {
            return;
        }
        let Some(source) = self.find_tab(tab) else {
            return;
        };
        let target = match drop {
            DockDrop::Into { group, .. } | DockDrop::Split { group, .. } => group,
        };
        if self.group(target).is_none() {
            return;
        }
        // Splitting a lone tab off its own group would empty the group
        // and re-split its collapsed remains — shape-preserving, skip.
        let source_len = self.group(source.group).expect("source exists").tabs.len();
        if source.group == target && source_len == 1 {
            return;
        }
        // Depth cap, checked before any mutation so a refused split
        // can't lose the already-removed tab (`target` was confirmed
        // above, so a `None` depth would be a bug, not a refusal).
        if matches!(drop, DockDrop::Split { .. }) {
            assert!(self.group_depth(target).is_some(), "target exists");
            if !self.can_split(target) {
                return;
            }
        }

        self.group_mut(source.group)
            .expect("source exists")
            .remove_tab(source.index);

        match drop {
            DockDrop::Into { group, index } => {
                // `index` addresses the strip as the caller saw it —
                // pre-move (that's what drop-zone math over the visible
                // chips produces). A rightward move within the same
                // group must compensate for its own removal.
                let index = if group == source.group && index > source.index {
                    index - 1
                } else {
                    index
                };
                let g = self.group_mut(group).expect("target exists");
                let index = index.min(g.tabs.len());
                g.tabs.insert(index, tab);
                g.active = index;
                self.focused = group;
            }
            DockDrop::Split { group, side } => {
                let new_group = TabGroup {
                    id: TabGroupId::unique(),
                    tabs: vec![tab],
                    active: 0,
                };
                self.focused = new_group.id;
                self.split_group(group, side, new_group);
            }
        }
        self.normalize();
    }

    /// Set the ratio of the split at `path`, clamped to the ratio
    /// bounds. A path that doesn't land on a split (the tree changed
    /// under a stale intent) is ignored.
    pub fn set_ratio(&mut self, path: DockPath, ratio: f32) {
        // A sentinel-less byte is a corrupt address, not the root —
        // ignore it like any other stale path.
        if path.0 == 0 {
            return;
        }
        let mut idx = Self::ROOT;
        for second in path.directions() {
            let DockNode::Split(s) = self.node(idx) else {
                return;
            };
            idx = if second { s.second } else { s.first };
        }
        if let DockNode::Split(s) = &mut self.nodes[idx.usize()] {
            s.ratio = ratio.clamp(RATIO_MIN, RATIO_MAX);
        }
    }

    /// Drop every tab failing `keep`, collapsing groups that empty —
    /// the layout half of `Document::ensure_valid_layout` pruning.
    pub fn retain_tabs(&mut self, mut keep: impl FnMut(TabRef) -> bool) {
        for node in &mut self.nodes {
            if let DockNode::Group(g) = node {
                g.tabs.retain(|t| keep(*t));
                g.clamp_active();
            }
        }
        self.normalize();
    }

    /// Whether `group`'s pane may still split (the [`MAX_SPLIT_DEPTH`]
    /// nesting cap) — lets the drag-drop UI skip offering edge zones
    /// that [`Self::move_tab`] would refuse anyway.
    pub fn can_split(&self, group: TabGroupId) -> bool {
        self.group_depth(group).is_some_and(|d| d < MAX_SPLIT_DEPTH)
    }

    /// Number of split ancestors above `id`'s group — what
    /// [`MAX_SPLIT_DEPTH`] caps.
    fn group_depth(&self, id: TabGroupId) -> Option<u32> {
        fn walk(l: &DockLayout, idx: NodeIdx, id: TabGroupId, depth: u32) -> Option<u32> {
            match l.node(idx) {
                DockNode::Group(g) => (g.id == id).then_some(depth),
                DockNode::Split(s) => {
                    walk(l, s.first, id, depth + 1).or_else(|| walk(l, s.second, id, depth + 1))
                }
            }
        }
        walk(self, Self::ROOT, id, 0)
    }

    /// Replace the `target` group's node with a split of it and
    /// `new_group` on `side`. The two children are parked at the vec's
    /// end; the caller's `normalize` re-packs to pre-order.
    fn split_group(&mut self, target: TabGroupId, side: SplitSide, new_group: TabGroup) {
        let Some(slot) = self
            .nodes
            .iter()
            .position(|n| matches!(n, DockNode::Group(g) if g.id == target))
        else {
            return;
        };
        let existing_idx = NodeIdx(self.nodes.len() as u32);
        let fresh_idx = NodeIdx(self.nodes.len() as u32 + 1);
        let (first, second) = if side.new_pane_first() {
            (fresh_idx, existing_idx)
        } else {
            (existing_idx, fresh_idx)
        };
        let existing = std::mem::replace(
            &mut self.nodes[slot],
            DockNode::Split(DockSplit {
                dir: side.dir(),
                ratio: 0.5,
                first,
                second,
            }),
        );
        self.nodes.push(existing);
        self.nodes.push(DockNode::Group(new_group));
    }

    /// Re-pack `nodes` into canonical pre-order from the root, dropping
    /// empty groups and dissolving splits left with one live child, then
    /// re-point a dangling focus at the primary group. The primary group
    /// always survives (`Main` never closes), so the root can't die.
    fn normalize(&mut self) {
        // Liveness per slot, bottom-up: a group lives while it has tabs,
        // a split while either child does.
        fn alive(nodes: &[DockNode], idx: NodeIdx) -> bool {
            match &nodes[idx.usize()] {
                DockNode::Group(g) => !g.tabs.is_empty(),
                DockNode::Split(s) => alive(nodes, s.first) || alive(nodes, s.second),
            }
        }
        // Pre-order copy of the live tree; a split with one live child
        // dissolves into that child in place.
        fn copy(src: &[DockNode], idx: NodeIdx, out: &mut Vec<DockNode>) -> NodeIdx {
            match &src[idx.usize()] {
                DockNode::Group(g) => {
                    out.push(DockNode::Group(g.clone()));
                    NodeIdx(out.len() as u32 - 1)
                }
                DockNode::Split(s) => match (alive(src, s.first), alive(src, s.second)) {
                    (true, true) => {
                        let slot = out.len();
                        // Reserve the parent's pre-order slot; children
                        // land right after.
                        out.push(DockNode::Split(*s));
                        let first = copy(src, s.first, out);
                        let second = copy(src, s.second, out);
                        out[slot] = DockNode::Split(DockSplit {
                            first,
                            second,
                            ..*s
                        });
                        NodeIdx(slot as u32)
                    }
                    (true, false) => copy(src, s.first, out),
                    (false, true) => copy(src, s.second, out),
                    (false, false) => unreachable!("a dead subtree is dissolved by its parent"),
                },
            }
        }
        assert!(
            alive(&self.nodes, Self::ROOT),
            "the primary group keeps the tree non-empty"
        );
        let mut out = Vec::with_capacity(self.nodes.len());
        copy(&self.nodes, Self::ROOT, &mut out);
        self.nodes = out;
        if self.group(self.focused).is_none() {
            self.focused = self.primary().id;
        }
    }

    /// Structural validation, in all builds — see the module doc for the
    /// invariant list. A deserialized layout is untrusted input, so a
    /// violation is a returned error, not a panic (indices are
    /// bounds-checked before any slot access). Graph-tab existence is the
    /// caller's ([`Document::check`] holds the graph).
    ///
    /// [`Document::check`]: crate::core::document::Document::check
    pub(crate) fn check(&self) -> Result<()> {
        // Canonical pre-order: walking the tree must visit exactly the
        // slots 0..len in order — this covers reachability, no dead
        // slots, and acyclicity in one sweep.
        fn walk(nodes: &[DockNode], idx: NodeIdx, depth: u32, expect: &mut u32) -> Result<()> {
            ensure!(
                idx.0 == *expect,
                "dock nodes are not in canonical pre-order"
            );
            ensure!(
                idx.usize() < nodes.len(),
                "dock node index {} out of range",
                idx.0
            );
            *expect += 1;
            if let DockNode::Split(s) = &nodes[idx.usize()] {
                ensure!(depth < MAX_SPLIT_DEPTH, "split nesting exceeds the cap");
                ensure!(
                    (RATIO_MIN..=RATIO_MAX).contains(&s.ratio),
                    "split ratio {} out of bounds",
                    s.ratio
                );
                walk(nodes, s.first, depth + 1, expect)?;
                walk(nodes, s.second, depth + 1, expect)?;
            }
            Ok(())
        }
        let mut expect = 0;
        walk(&self.nodes, Self::ROOT, 0, &mut expect)?;
        ensure!(
            expect as usize == self.nodes.len(),
            "dock tree has slots unreachable from the root"
        );

        // Resolved by hand rather than via `primary()`, which `expect`s —
        // a corrupt layout may hold no Main tab at all.
        let primary = self
            .groups()
            .find(|g| g.tabs.contains(&TabRef::Graph(GraphRef::Main)))
            .context("no group holds the Main graph tab")?;
        let mut seen = Vec::new();
        let mut seen_groups = Vec::new();
        for g in self.groups() {
            // Group ids address every layout op (`group`/`group_mut` take the
            // first match), so a duplicate silently retargets ops.
            ensure!(
                !seen_groups.contains(&g.id),
                "dock group id {:?} appears twice",
                g.id
            );
            seen_groups.push(g.id);
            ensure!(!g.tabs.is_empty(), "dock group {:?} is empty", g.id);
            ensure!(
                g.active < g.tabs.len(),
                "dock group {:?} active tab out of range",
                g.id
            );
            for tab in &g.tabs {
                ensure!(!seen.contains(tab), "tab {tab:?} appears twice");
                seen.push(*tab);
                if matches!(tab, TabRef::Graph(_)) {
                    ensure!(
                        g.id == primary.id,
                        "graph tab {tab:?} lives outside the primary group"
                    );
                }
            }
        }
        ensure!(
            self.group(self.focused).is_some(),
            "focused group {:?} is missing",
            self.focused
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::document::{PortKind, PortRef};
    use scenarium::NodeId;

    fn viewer(n: u128) -> TabRef {
        TabRef::ImageViewer(PortRef {
            node_id: NodeId::from_u128(n),
            kind: PortKind::Output,
            port_idx: 0,
        })
    }

    fn main_tab() -> TabRef {
        TabRef::Graph(GraphRef::Main)
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
        l.check().unwrap();
        assert_eq!(l.groups().count(), 1);
        assert_eq!(l.primary().tabs, [main_tab()]);
        assert_eq!(l.focused, l.primary().id);
        assert_eq!(l.all_tabs().collect::<Vec<_>>(), [main_tab()]);
    }

    #[test]
    fn split_move_and_collapse_roundtrip() {
        let mut l = seeded();
        let primary = l.primary().id;

        // Split the viewer off to the right: a Row split, primary first,
        // the new single-tab group second, focused. The re-packed vec is
        // pre-order: [split, primary, new] — check pins that shape.
        l.move_tab(
            viewer(1),
            DockDrop::Split {
                group: primary,
                side: SplitSide::Right,
            },
        );
        l.check().unwrap();
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
        l.check().unwrap();
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
            l.check().unwrap();
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

        // Graph tabs are pinned to the primary group (phase-1 rule).
        l.move_tab(
            main_tab(),
            DockDrop::Split {
                group: lone,
                side: SplitSide::Right,
            },
        );
        assert_eq!(l, before, "graph tabs never move");

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
        let lone = l.focused;

        l.close_tab(lone, 0);
        l.check().unwrap();
        assert!(
            matches!(l.node(DockLayout::ROOT), DockNode::Group(_)),
            "empty pane collapsed"
        );
        assert_eq!(l.focused, primary, "dangling focus falls back to primary");

        // Main never closes, directly or by index games.
        l.close_tab(primary, 0);
        assert_eq!(l.primary().tabs[0], main_tab());
        l.close_tab(primary, 99);
        l.check().unwrap();
    }

    #[test]
    fn close_keeps_active_on_the_surviving_tab() {
        let mut l = seeded();
        let primary = l.primary().id;
        l.activate(primary, 2);
        assert_eq!(l.group(l.focused).unwrap().active_tab(), viewer(1));

        // Closing the active last tab clamps active onto the previous one.
        l.close_tab(primary, 2);
        l.check().unwrap();
        assert_eq!(l.primary().tabs, [main_tab(), TabRef::Preferences]);
        assert_eq!(l.primary().active, 1);

        // Closing a tab left of the active one shifts what `active`
        // points at — the old flat strip recomputed the index; per-group
        // clamping keeps it in range (pointing at the same slot is
        // accepted drift, the snapshot undo restores exact state).
        let mut l = seeded();
        l.activate(primary, 2);
        l.close_tab(primary, 1);
        l.check().unwrap();
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
            l.check().unwrap();
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
        l.check().unwrap();
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
        l.check().unwrap();
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
        l.check().unwrap();
        assert!(matches!(l.node(DockLayout::ROOT), DockNode::Group(_)));
        assert_eq!(
            l.all_tabs().collect::<Vec<_>>(),
            [main_tab(), TabRef::Preferences, viewer(1)]
        );
    }

    #[test]
    fn nested_splits_stay_canonical() {
        // Two nested splits: [rootsplit, primary, innersplit, v1, v2] in
        // pre-order once packed — check's walk pins the exact shape,
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
        l.check().unwrap();
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
        l.check().unwrap();
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
    fn serde_roundtrips_through_rhai() {
        let mut l = seeded();
        l.move_tab(
            viewer(1),
            DockDrop::Split {
                group: l.primary().id,
                side: SplitSide::Bottom,
            },
        );
        let bytes = common::serialize(&l, common::SerdeFormat::Rhai).unwrap();
        let back: DockLayout = common::deserialize(&bytes, common::SerdeFormat::Rhai).unwrap();
        assert_eq!(back, l);
    }

    #[test]
    fn check_rejects_each_corruption() {
        use scenarium::SubgraphId;

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
            l.check().unwrap();
            l
        };

        type Corrupt = fn(&mut DockLayout);
        let cases: [(&str, Corrupt, &str); 9] = [
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
            (
                "graph tab outside the primary group",
                |l| {
                    let DockNode::Group(g) = &mut l.nodes[2] else {
                        panic!("slot 2 is the split-off viewer pane");
                    };
                    g.tabs
                        .push(TabRef::Graph(GraphRef::Local(SubgraphId::from_u128(7))));
                },
                "outside the primary group",
            ),
        ];
        for (name, corrupt, expected) in cases {
            let mut l = base.clone();
            corrupt(&mut l);
            let err = l.check().unwrap_err().to_string();
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
        let err = l.check().unwrap_err().to_string();
        assert!(
            err.contains("no group holds the Main graph tab"),
            "missing Main: unexpected error: {err}"
        );
    }
}
