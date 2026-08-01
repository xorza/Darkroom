//! The dock layout: a binary split tree whose leaves are tab groups.
//! Replaces the old flat tab strip — each [`TabGroup`] renders as one
//! pane with its own strip, and a [`DockSplit`] divides the space
//! between two child nodes at a draggable `ratio`. Pure data + pure
//! ops: every mutation is a [`DockOp`] applied in place, reporting
//! nothing. None of them is undoable — rearranging panes is navigation,
//! so Ctrl+Z walks past it to the last graph edit.
//!
//! **Flat storage.** The tree lives in one `Vec<DockNode>` with
//! [`NodeIdx`] children — no per-node boxes. The vec is kept
//! *canonical*: pre-order from the root at slot 0, no dead slots
//! ([`DockLayout::normalize`] re-packs after every structural op). That
//! makes `Vec` equality structural equality (the undo layer's no-op
//! diff depends on it) and group iteration a plain vec scan in
//! left-to-right pane order.
//!
//! Invariants (checked by [`DockLayout::validate`]):
//! - the vec is canonical pre-order, fully reachable from slot 0;
//! - exactly one group holds the `Main` graph tab (the *primary* group,
//!   successor of the old `tabs[0] is Main` rule);
//! - no group is empty, no tab appears twice, group ids are unique,
//!   per-group `active` is in range, `focused` names a live group,
//!   ratios stay in `RATIO_MIN..=RATIO_MAX`.
//!
//! Graph tabs are *not* pinned to the primary group: any pane can show
//! any graph, and every pane showing one gets its own canvas (see
//! `gui::pane::graph::GraphUI`). `Main` still can't be closed, which is what
//! keeps the primary group — and so the tree — alive.

use common::id_type;
use serde::{Deserialize, Serialize};

use crate::core::document::TabRef;

id_type!(TabGroupId);

/// Split-ratio clamp: neither pane can be squeezed below a tenth of the
/// split, so a divider can't be dragged into an unrecoverable sliver.
const RATIO_MIN: f32 = 0.1;
const RATIO_MAX: f32 = 0.9;

/// Most nested splits allowed on any root-to-leaf chain (up to 16
/// panes) — [`DockLayout::move_tab`] refuses splits past it. Keeps the
/// UI sane and every split address comfortably inside a [`DockPath`].
const MAX_SPLIT_DEPTH: u32 = 4;

#[derive(Debug, thiserror::Error)]
pub(crate) enum DockValidationError {
    #[error("dock nodes are not in canonical pre-order")]
    NonCanonical,
    #[error("dock node index {index} out of range")]
    NodeOutOfRange { index: u32 },
    #[error("split nesting exceeds the cap")]
    SplitNesting,
    #[error("split ratio {ratio} out of bounds")]
    SplitRatio { ratio: f32 },
    #[error("dock tree has slots unreachable from the root")]
    UnreachableSlots,
    #[error("no group holds the Main graph tab")]
    MissingMainTab,
    #[error("dock group id {group_id:?} appears twice")]
    DuplicateGroup { group_id: TabGroupId },
    #[error("dock group {group_id:?} is empty")]
    EmptyGroup { group_id: TabGroupId },
    #[error("dock group {group_id:?} active tab out of range")]
    ActiveTabOutOfRange { group_id: TabGroupId },
    #[error("tab {tab:?} appears twice")]
    DuplicateTab { tab: TabRef },
    #[error("focused group {group_id:?} is missing")]
    MissingFocusedGroup { group_id: TabGroupId },
}

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
pub(crate) struct DockPath(u8);

impl DockPath {
    /// The root node's address (the empty path).
    pub(crate) const ROOT: DockPath = DockPath(1);

    /// The address of `self`'s first (left/top) child.
    pub(crate) fn first(self) -> DockPath {
        self.child(false)
    }

    /// The address of `self`'s second (right/bottom) child.
    pub(crate) fn second(self) -> DockPath {
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
pub(crate) struct NodeIdx(u32);

impl NodeIdx {
    fn usize(self) -> usize {
        self.0 as usize
    }
}

/// How a [`DockSplit`] arranges its children: `Row` side by side
/// (vertical divider), `Column` stacked (horizontal divider).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum SplitDir {
    Row,
    Column,
}

/// Which edge of a pane a split lands on — the new pane takes that
/// edge's half. `Left`/`Right` split into a [`SplitDir::Row`],
/// `Top`/`Bottom` into a [`SplitDir::Column`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum SplitSide {
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
pub(crate) enum DockDrop {
    /// Join `group`'s strip at `index` (clamped to its length).
    Into { group: TabGroupId, index: usize },
    /// Split `group`'s pane; the tab becomes a fresh single-tab group on
    /// the given side.
    Split { group: TabGroupId, side: SplitSide },
}

/// One dock-layout mutation, executed by [`DockLayout::apply`]. The
/// single op vocabulary the whole pipeline speaks: the dock UI (or a
/// menu item, or a preview card's chip) constructs one, the frame's
/// queue transports it as `DocumentRequest::View`, and `apply` runs it.
///
/// **Every op tolerates a stale address.** One is built from a response
/// of the frame before and applied a phase later, by which time the tab,
/// group, or split it names may be gone — so an op that resolves to
/// nothing leaves the layout untouched rather than failing.
///
/// Every tab op names its tab by identity, never by strip position: an
/// index would by then address whatever tab slid into that slot.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum DockOp {
    /// Make `tab` visible in whichever group holds it, and focus that
    /// group.
    ActivateTab { tab: TabRef },
    /// Open `tab` in the focused group — reusing it wherever it already
    /// sits — then make it visible and focus its pane. The whole of "show
    /// me X": the Preferences menu item and a preview card's viewer chip
    /// both raise this and nothing else.
    OpenTab { tab: TabRef },
    /// Close `tab` wherever it sits. The `Main` tab never closes — the
    /// op refuses it.
    CloseTab { tab: TabRef },
    /// Move `tab` to `to` — into another strip or splitting a pane.
    MoveTab { tab: TabRef, to: DockDrop },
    /// Set the ratio of the split at `split` (its packed root path).
    /// Emitted per frame by a divider drag; coalesces per split.
    SetRatio { split: DockPath, ratio: f32 },
    /// Move focus onto `group`, because a press landed inside its pane.
    /// The incidental half of navigation — focus following the pointer —
    /// beside the deliberate ops around it.
    FocusPane { group: TabGroupId },
}

/// One pane's tab strip: the open tabs plus which one is visible.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct TabGroup {
    pub(crate) id: TabGroupId,
    /// Non-empty; a group whose last tab closes collapses out of the tree.
    pub(crate) tabs: Vec<TabRef>,
    /// Index of the visible tab; always in range.
    pub(crate) active: usize,
}

impl TabGroup {
    pub(crate) fn active_tab(&self) -> TabRef {
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
pub(crate) enum DockNode {
    Split(DockSplit),
    Group(TabGroup),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DockSplit {
    pub(crate) dir: SplitDir,
    /// The first child's share of the free space, in
    /// `RATIO_MIN..=RATIO_MAX`.
    pub(crate) ratio: f32,
    pub(crate) first: NodeIdx,
    pub(crate) second: NodeIdx,
}

/// The whole pane arrangement: the flat split tree plus which group has
/// focus (keyboard-shortcut routing + where opened tabs land). Persisted
/// on the `Document` and snapshot into every dock undo step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DockLayout {
    /// Canonical pre-order (see the module doc). Private so every
    /// structural mutation goes through the ops that renormalize.
    nodes: Vec<DockNode>,
    pub(crate) focused: TabGroupId,
}

impl Default for DockLayout {
    /// A single group holding the `Main` graph. `nil` keys the default
    /// primary group deterministically (defaults compare equal); split
    /// offspring get `unique()` ids.
    fn default() -> Self {
        let primary = TabGroup {
            id: TabGroupId::nil(),
            tabs: vec![TabRef::Graph],
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
pub(crate) struct TabAddress {
    pub(crate) group: TabGroupId,
    pub(crate) index: usize,
}

impl DockLayout {
    /// The root node's index — always slot 0 in the canonical order.
    pub(crate) const ROOT: NodeIdx = NodeIdx(0);

    /// The node at `idx` — the render walk follows [`DockSplit`]'s child
    /// indices through this.
    pub(crate) fn node(&self, idx: NodeIdx) -> &DockNode {
        &self.nodes[idx.usize()]
    }

    /// The leaf groups in left-to-right, top-to-bottom pane order — in
    /// canonical pre-order storage that's simply vec order.
    pub(crate) fn groups(&self) -> impl Iterator<Item = &TabGroup> {
        self.nodes.iter().filter_map(|n| match n {
            DockNode::Group(g) => Some(g),
            DockNode::Split(_) => None,
        })
    }

    /// Every open tab across every group, in [`Self::groups`] order.
    pub(crate) fn all_tabs(&self) -> impl Iterator<Item = TabRef> + '_ {
        self.groups().flat_map(|g| g.tabs.iter().copied())
    }

    /// What each pane is showing — one tab per group, in [`Self::groups`]
    /// order. The frame's per-pane passes iterate this: a pane's own logic is
    /// keyed to the tab it currently displays, not to every tab it holds.
    pub(crate) fn active_tabs(&self) -> impl Iterator<Item = TabRef> + '_ {
        self.groups().map(TabGroup::active_tab)
    }

    fn group(&self, id: TabGroupId) -> Option<&TabGroup> {
        self.groups().find(|g| g.id == id)
    }

    /// Move focus onto `group` — the pane a press landed in.
    ///
    /// A group that has gone since the press no-ops, like every other op fed
    /// a stale address: storing a dead id would strand `focused` and fail
    /// [`Self::validate`] at the next save.
    fn focus(&mut self, group: TabGroupId) {
        if self.group(group).is_some() {
            self.focused = group;
        }
    }

    fn group_mut(&mut self, id: TabGroupId) -> Option<&mut TabGroup> {
        self.nodes.iter_mut().find_map(|n| match n {
            DockNode::Group(g) if g.id == id => Some(g),
            _ => None,
        })
    }

    /// The group holding the `Main` graph tab — the one pane that hosts
    /// graph canvases.
    pub(crate) fn primary(&self) -> &TabGroup {
        self.groups()
            .find(|g| g.tabs.contains(&TabRef::Graph))
            .expect("a group holds the Main tab")
    }

    pub(crate) fn find_tab(&self, tab: TabRef) -> Option<TabAddress> {
        self.groups().find_map(|g| {
            g.tabs
                .iter()
                .position(|t| *t == tab)
                .map(|index| TabAddress { group: g.id, index })
        })
    }

    /// Execute one [`DockOp`] — the dispatch behind every recorded
    /// layout mutation.
    pub(crate) fn apply(&mut self, op: DockOp) {
        match op {
            DockOp::ActivateTab { tab } => self.activate(tab),
            DockOp::OpenTab { tab } => self.open_tab(tab),
            DockOp::CloseTab { tab } => self.close_tab(tab),
            DockOp::MoveTab { tab, to } => self.move_tab(tab, to),
            DockOp::SetRatio { split, ratio } => self.set_ratio(split, ratio),
            DockOp::FocusPane { group } => self.focus(group),
        }
    }

    /// Add `tab` to the focused group unless it is already open somewhere,
    /// then activate it — which also focuses whichever pane ended up
    /// holding it.
    fn open_tab(&mut self, tab: TabRef) {
        self.find_or_insert(tab, self.focused);
        self.activate(tab);
    }

    /// Make `tab` the visible one in whichever group holds it, and focus
    /// that group. A tab that has since closed no-ops.
    fn activate(&mut self, tab: TabRef) {
        let Some(TabAddress { group, index }) = self.find_tab(tab) else {
            return;
        };
        self.group_mut(group)
            .expect("find_tab resolved a live group")
            .active = index;
        self.focused = group;
    }

    /// Append `tab` to `group`'s strip unless it's already open somewhere —
    /// the half of [`DockOp::OpenTab`] that puts the tab in the tree, without
    /// the activation that follows. Unlike the queued ops this is a direct
    /// call whose callers name a group they hold live (`open_tab` passes
    /// `focused`), so a dead id is a logic error, not tolerable staleness.
    pub(crate) fn find_or_insert(&mut self, tab: TabRef, group: TabGroupId) {
        if self.find_tab(tab).is_none() {
            self.insert_tab(group, tab);
        }
    }

    /// Raw append of `tab` to `group`'s strip; [`Self::find_or_insert`]
    /// owns the dedup.
    fn insert_tab(&mut self, group: TabGroupId, tab: TabRef) {
        self.group_mut(group)
            .expect("insert target group exists")
            .tabs
            .push(tab);
    }

    /// Close `tab` wherever it sits. The `Main` tab never closes. A group
    /// emptied by the close collapses out of the tree; a vanished focus
    /// falls back to the primary group.
    fn close_tab(&mut self, tab: TabRef) {
        if tab == TabRef::Graph {
            return;
        }
        let Some(TabAddress { group, index }) = self.find_tab(tab) else {
            return;
        };
        self.group_mut(group)
            .expect("find_tab resolved a live group")
            .remove_tab(index);
        self.normalize();
    }

    /// Move `tab` to `drop`, collapsing whatever its departure empties.
    /// An `Into` index addresses the target strip *as the caller saw
    /// it* (pre-move) — a reorder within one group lands exactly where
    /// the drop-zone math over the visible chips said, despite the
    /// tab's own removal shifting the slots. The destination group
    /// (fresh one for a split) takes the tab as its active and gains
    /// focus. Degenerate moves — a split off a group that holds only
    /// this tab, targeting itself — leave the layout unchanged (the
    /// snapshot diff drops them).
    ///
    /// Graph tabs move like any other: splitting one off is how two
    /// graphs end up side by side.
    fn move_tab(&mut self, tab: TabRef, drop: DockDrop) {
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
    fn set_ratio(&mut self, path: DockPath, ratio: f32) {
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
    /// the layout half of `Document::reconcile_with_graph` pruning.
    pub(crate) fn retain_tabs(&mut self, mut keep: impl FnMut(TabRef) -> bool) {
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
    pub(crate) fn can_split(&self, group: TabGroupId) -> bool {
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
    /// caller's ([`Document::validate`] holds the graph).
    ///
    /// [`Document::validate`]: crate::core::document::Document::validate
    pub(super) fn validate(&self) -> Result<(), DockValidationError> {
        // Canonical pre-order: walking the tree must visit exactly the
        // slots 0..len in order — this covers reachability, no dead
        // slots, and acyclicity in one sweep.
        fn walk(
            nodes: &[DockNode],
            idx: NodeIdx,
            depth: u32,
            expect: &mut u32,
        ) -> Result<(), DockValidationError> {
            if idx.0 != *expect {
                return Err(DockValidationError::NonCanonical);
            }
            if idx.usize() >= nodes.len() {
                return Err(DockValidationError::NodeOutOfRange { index: idx.0 });
            }
            *expect += 1;
            if let DockNode::Split(s) = &nodes[idx.usize()] {
                if depth >= MAX_SPLIT_DEPTH {
                    return Err(DockValidationError::SplitNesting);
                }
                if !(RATIO_MIN..=RATIO_MAX).contains(&s.ratio) {
                    return Err(DockValidationError::SplitRatio { ratio: s.ratio });
                }
                walk(nodes, s.first, depth + 1, expect)?;
                walk(nodes, s.second, depth + 1, expect)?;
            }
            Ok(())
        }
        let mut expect = 0;
        walk(&self.nodes, Self::ROOT, 0, &mut expect)?;
        if expect as usize != self.nodes.len() {
            return Err(DockValidationError::UnreachableSlots);
        }

        // Resolved by hand rather than via `primary()`, which `expect`s —
        // a corrupt layout may hold no Main tab at all.
        self.groups()
            .find(|g| g.tabs.contains(&TabRef::Graph))
            .ok_or(DockValidationError::MissingMainTab)?;
        let mut seen = Vec::new();
        let mut seen_groups = Vec::new();
        for g in self.groups() {
            // Group ids address every layout op (`group`/`group_mut` take the
            // first match), so a duplicate silently retargets ops.
            if seen_groups.contains(&g.id) {
                return Err(DockValidationError::DuplicateGroup { group_id: g.id });
            }
            seen_groups.push(g.id);
            if g.tabs.is_empty() {
                return Err(DockValidationError::EmptyGroup { group_id: g.id });
            }
            if g.active >= g.tabs.len() {
                return Err(DockValidationError::ActiveTabOutOfRange { group_id: g.id });
            }
            for tab in &g.tabs {
                if seen.contains(tab) {
                    return Err(DockValidationError::DuplicateTab { tab: *tab });
                }
                seen.push(*tab);
            }
        }
        if self.group(self.focused).is_none() {
            return Err(DockValidationError::MissingFocusedGroup {
                group_id: self.focused,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
