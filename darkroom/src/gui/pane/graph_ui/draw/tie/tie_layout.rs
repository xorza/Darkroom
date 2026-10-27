//! [`TieLayout`]: each tie's band and each tied wire's pieces, for one frame.

use std::f32::consts::FRAC_1_SQRT_2;
use std::ops::Range;

use glam::Vec2;

use crate::core::document::graph_view::GraphView;
use crate::core::document::tie::{TieAngle, TieId};
use crate::core::document::wire_key::{DataWire, WireKey};
use crate::gui::pane::graph_ui::draw::wire::Wire;
use crate::gui::pane::graph_ui::frame::canvas_geometry::CanvasGeometry;
use crate::gui::port_ref::PortRef;

/// World units between two strands of a band, and the band's length past its outer strands.
pub(crate) const SLOT_SPACING: f32 = 9.0;

/// Slot passes: the first orders each band from its neighbours' tie centres, the later ones from
/// the slots the pass before gave them, so the bands of chained ties settle on one order.
const SLOT_PASSES: usize = 3;

/// A tied piece's handles are this share of its chord, within [`MIN_HANDLE`, `MAX_HANDLE`].
const HANDLE_SHARE: f32 = 0.4;
const MIN_HANDLE: f32 = 24.0;
const MAX_HANDLE: f32 = 140.0;

/// The way an event wire meets its subscriber pin, down-right, as [`Wire::event`]'s up-left
/// handle has it.
const EVENT_ARRIVAL: Vec2 = Vec2::new(FRAC_1_SQRT_2, FRAC_1_SQRT_2);

/// Where each tie's band stands this frame and where each tied wire runs through it, rebuilt
/// each frame into buffers it keeps, so a settled frame allocates nothing.
///
/// A strand's slot is derived, never stored: the barycenter of its two neighbouring points along
/// the band axis orders the band, the crossing-reduction heuristic of layered graph drawing
/// (Sugiyama, Tagawa, Toda 1981). `darkroom/AGENTS.md`, "Wire ties", "A route is stored, a strand's
/// slot is derived", holds the decision.
#[derive(Debug, Default)]
pub(crate) struct TieLayout {
    /// Every tie of the view, in id order.
    bands: Vec<Band>,
    /// Every routed wire whose two ends the canvas places, in key order.
    wires: Vec<RoutedWire>,
    /// Each strand through a band, grouped by band: [`Band::strands`] indexes it.
    strands: Vec<Strand>,
    /// For step `k` of wire `w`, the index into [`Self::strands`] of its strand, at
    /// `wires[w].steps.start + k`.
    step_strand: Vec<u32>,
    /// Each strand's slot, by the strand's index.
    slots: Vec<Vec2>,
    /// The slots the pass in progress writes, swapped with [`Self::slots`] after it.
    next_slots: Vec<Vec2>,
    /// One band's strands with the value that orders them, sorted in place each pass.
    order: Vec<Ordered>,
    /// Each routed wire's cubics, from its output end to its input end.
    pieces: Vec<Wire>,
}

/// One tie's band this frame.
#[derive(Clone, Debug)]
pub(crate) struct Band {
    pub(crate) tie: TieId,
    pub(crate) angle: TieAngle,
    pub(crate) centre: Vec2,
    /// The way the strands run through the band, a unit vector.
    pub(crate) dir: Vec2,
    /// The band's own direction, square to [`Self::dir`]: `dir.perp()`.
    pub(crate) axis: Vec2,
    /// Its strands, a range of [`TieLayout::strands`]; empty when the canvas places none of its
    /// wires' ends yet.
    strands: Range<u32>,
}

impl Band {
    /// How many strands run through the band.
    pub(crate) const fn strand_count(&self) -> u32 {
        self.strands.end - self.strands.start
    }
}

/// A wire with a route, and where its pieces are.
#[derive(Clone, Debug)]
struct RoutedWire {
    key: WireKey,
    start: Vec2,
    end: Vec2,
    /// The way the wire leaves its output end and meets its input end.
    leaves: Vec2,
    arrives: Vec2,
    /// Its route's steps, a range of [`TieLayout::step_strand`].
    steps: Range<u32>,
    /// Its cubics, a range of [`TieLayout::pieces`].
    pieces: Range<u32>,
}

/// Step `step` of wire `wire`, through band `band`.
#[derive(Clone, Copy, Debug)]
struct Strand {
    band: u32,
    wire: u32,
    step: u32,
}

/// A strand and the barycenter that orders it in its band.
#[derive(Clone, Copy, Debug)]
struct Ordered {
    along: f32,
    key: WireKey,
    strand: u32,
}

/// Where a wire's two ends sit: its output's or emitter's port, and its input's port or
/// subscriber's pin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Span {
    pub(crate) start: Vec2,
    pub(crate) end: Vec2,
}

impl Span {
    /// `wire`'s ends as `geometry` places them, or `None` while a port has not recorded yet.
    pub(crate) fn of(geometry: &CanvasGeometry, wire: WireKey) -> Option<Self> {
        let (start, end) = match wire {
            WireKey::Data(DataWire { output, input }) => (
                geometry.ports.center(PortRef::from(output))?,
                geometry.ports.center(PortRef::from(input))?,
            ),
            WireKey::Event(subscription) => (
                geometry.events.center(subscription.event)?,
                geometry.subs.center(subscription.subscriber)?,
            ),
        };
        Some(Self { start, end })
    }
}

impl TieLayout {
    /// Lay out `view`'s ties, with each wire's ends where `span` puts them.
    pub(crate) fn rebuild(&mut self, view: &GraphView, span: impl Fn(WireKey) -> Option<Span>) {
        self.clear();
        self.bands
            .extend(view.ties.iter().map(|(&tie, placed)| Band {
                tie,
                angle: placed.angle,
                centre: placed.pos,
                dir: Vec2::X,
                axis: Vec2::Y,
                strands: 0..0,
            }));
        for (&key, route) in &view.routes {
            let Some(Span { start, end }) = span(key) else {
                continue;
            };
            let wire = u32::try_from(self.wires.len()).expect("a frame's wires fit a u32");
            let first = self.step_strand.len() as u32;
            for (step, tie) in route.iter().enumerate() {
                let band = self
                    .bands
                    .binary_search_by_key(tie, |band| band.tie)
                    .expect("a valid view's routes name its ties");
                self.strands.push(Strand {
                    band: band as u32,
                    wire,
                    step: step as u32,
                });
                self.step_strand.push(0);
            }
            let (leaves, arrives) = match key {
                WireKey::Data(_) => (Vec2::X, Vec2::X),
                WireKey::Event(_) => (Vec2::X, EVENT_ARRIVAL),
            };
            self.wires.push(RoutedWire {
                key,
                start,
                end,
                leaves,
                arrives,
                steps: first..self.step_strand.len() as u32,
                pieces: 0..0,
            });
        }
        self.group_strands();
        self.turn_bands();
        self.place_slots();
        self.lay_pieces();
    }

    /// Each band, in tie id order.
    pub(crate) fn bands(&self) -> &[Band] {
        &self.bands
    }

    /// The band of `tie`.
    pub(crate) fn band(&self, tie: TieId) -> Option<&Band> {
        let idx = self
            .bands
            .binary_search_by_key(&tie, |band| band.tie)
            .ok()?;
        Some(&self.bands[idx])
    }

    /// The cubics `wire` runs as, from its output end to its input end, or `None` for a wire
    /// with no route or with an end the canvas does not place.
    pub(crate) fn pieces(&self, wire: WireKey) -> Option<&[Wire]> {
        let idx = self
            .wires
            .binary_search_by_key(&wire, |routed| routed.key)
            .ok()?;
        Some(&self.pieces[range(&self.wires[idx].pieces)])
    }

    fn clear(&mut self) {
        let Self {
            bands,
            wires,
            strands,
            step_strand,
            slots,
            next_slots,
            order,
            pieces,
        } = self;
        bands.clear();
        wires.clear();
        strands.clear();
        step_strand.clear();
        slots.clear();
        next_slots.clear();
        order.clear();
        pieces.clear();
    }

    /// Sort the strands by band, give each band its range, and point each step at its strand.
    fn group_strands(&mut self) {
        self.strands
            .sort_unstable_by_key(|strand| (strand.band, strand.wire, strand.step));
        let mut idx = 0;
        for (band_idx, band) in self.bands.iter_mut().enumerate() {
            let start = idx;
            while self
                .strands
                .get(idx)
                .is_some_and(|strand| strand.band as usize == band_idx)
            {
                idx += 1;
            }
            band.strands = start as u32..idx as u32;
        }
        for (strand_idx, strand) in self.strands.iter().enumerate() {
            let at = self.wires[strand.wire as usize].steps.start + strand.step;
            self.step_strand[at as usize] = strand_idx as u32;
        }
    }

    /// Point each band's strands: a pinned angle, or the mean direction of its strands' chords
    /// between their neighbouring tie centres or ends.
    fn turn_bands(&mut self) {
        for band_idx in 0..self.bands.len() {
            let band = &self.bands[band_idx];
            let dir = match band.angle {
                TieAngle::Pinned(degrees) => {
                    let (sin, cos) = degrees.to_radians().sin_cos();
                    Vec2::new(sin, -cos)
                }
                TieAngle::Auto => self.strands[range(&band.strands)]
                    .iter()
                    .map(|strand| {
                        (self.after(*strand, None) - self.before(*strand, None)).normalize_or_zero()
                    })
                    .sum::<Vec2>()
                    .try_normalize()
                    .unwrap_or(Vec2::X),
            };
            let band = &mut self.bands[band_idx];
            band.dir = dir;
            band.axis = dir.perp();
        }
    }

    /// Give each strand its slot: ordered in its band by the barycenter of its two neighbouring
    /// points along the axis, equal ones by wire key, [`SLOT_SPACING`] apart and centred.
    fn place_slots(&mut self) {
        let count = self.strands.len();
        self.slots.resize(count, Vec2::ZERO);
        self.next_slots.resize(count, Vec2::ZERO);
        for pass in 0..SLOT_PASSES {
            let use_slots = pass > 0;
            for band_idx in 0..self.bands.len() {
                let band = &self.bands[band_idx];
                self.order.clear();
                for strand_idx in range(&band.strands) {
                    let strand = self.strands[strand_idx];
                    let slots = use_slots.then_some(self.slots.as_slice());
                    let mid = (self.before(strand, slots) + self.after(strand, slots)) * 0.5;
                    self.order.push(Ordered {
                        along: (mid - band.centre).dot(band.axis),
                        key: self.wires[strand.wire as usize].key,
                        strand: strand_idx as u32,
                    });
                }
                self.order.sort_unstable_by(|a, b| {
                    a.along.total_cmp(&b.along).then_with(|| a.key.cmp(&b.key))
                });
                let middle = (self.order.len() as f32 - 1.0) * 0.5;
                for (rank, ordered) in self.order.iter().enumerate() {
                    self.next_slots[ordered.strand as usize] =
                        band.centre + band.axis * ((rank as f32 - middle) * SLOT_SPACING);
                }
            }
            std::mem::swap(&mut self.slots, &mut self.next_slots);
        }
    }

    /// Each routed wire's cubics: port to tie, tie to tie, tie to port. At a tie the tangent runs
    /// along the band's strand direction, signed by the wire's travel, so the curve is G1 there;
    /// a ribbon piece between two ties takes its handles from the chord between their centres, so
    /// its strands share their handles.
    fn lay_pieces(&mut self) {
        for wire_idx in 0..self.wires.len() {
            let first = self.pieces.len() as u32;
            let steps = self.wires[wire_idx].steps.end - self.wires[wire_idx].steps.start;
            for piece in 0..=steps {
                let from = self.point(wire_idx, piece);
                let to = self.point(wire_idx, piece + 1);
                let chord = if piece > 0 && piece < steps {
                    self.centre_of_step(wire_idx, piece)
                        .distance(self.centre_of_step(wire_idx, piece - 1))
                } else {
                    from.distance(to)
                };
                let handle = (chord * HANDLE_SHARE).clamp(MIN_HANDLE, MAX_HANDLE);
                self.pieces.push(Wire {
                    p0: from,
                    p1: from + self.tangent(wire_idx, piece) * handle,
                    p2: to - self.tangent(wire_idx, piece + 1) * handle,
                    p3: to,
                });
            }
            self.wires[wire_idx].pieces = first..self.pieces.len() as u32;
        }
    }

    /// Point `idx` of a wire: its start at `0`, the slot of step `idx - 1` between, its end last.
    fn point(&self, wire_idx: usize, idx: u32) -> Vec2 {
        let wire = &self.wires[wire_idx];
        let steps = wire.steps.end - wire.steps.start;
        match idx {
            0 => wire.start,
            idx if idx > steps => wire.end,
            idx => self.slots[self.step_strand[(wire.steps.start + idx - 1) as usize] as usize],
        }
    }

    /// The way a wire runs at its point `idx`: along the band at a slot, toward the wire's
    /// next point.
    fn tangent(&self, wire_idx: usize, idx: u32) -> Vec2 {
        let wire = &self.wires[wire_idx];
        let steps = wire.steps.end - wire.steps.start;
        match idx {
            0 => wire.leaves,
            idx if idx > steps => wire.arrives,
            idx => {
                let band = &self.bands[self.strand_at(wire_idx, idx - 1).band as usize];
                let travel = self.point(wire_idx, idx + 1) - self.point(wire_idx, idx - 1);
                if travel.dot(band.dir) < 0.0 {
                    -band.dir
                } else {
                    band.dir
                }
            }
        }
    }

    /// The centre of the tie at step `step` of a wire.
    fn centre_of_step(&self, wire_idx: usize, step: u32) -> Vec2 {
        self.bands[self.strand_at(wire_idx, step).band as usize].centre
    }

    fn strand_at(&self, wire_idx: usize, step: u32) -> Strand {
        let at = self.wires[wire_idx].steps.start + step;
        self.strands[self.step_strand[at as usize] as usize]
    }

    /// The point before `strand` on its wire: the wire's start, or the previous tie's slot when
    /// `slots` holds them, else its centre.
    fn before(&self, strand: Strand, slots: Option<&[Vec2]>) -> Vec2 {
        let wire = &self.wires[strand.wire as usize];
        if strand.step == 0 {
            return wire.start;
        }
        self.neighbour(strand.wire, strand.step - 1, slots)
    }

    /// The point after `strand` on its wire: the wire's end, or the next tie's slot or centre.
    fn after(&self, strand: Strand, slots: Option<&[Vec2]>) -> Vec2 {
        let wire = &self.wires[strand.wire as usize];
        if strand.step + 1 == wire.steps.end - wire.steps.start {
            return wire.end;
        }
        self.neighbour(strand.wire, strand.step + 1, slots)
    }

    fn neighbour(&self, wire: u32, step: u32, slots: Option<&[Vec2]>) -> Vec2 {
        let strand_idx = self.step_strand[(self.wires[wire as usize].steps.start + step) as usize];
        match slots {
            Some(slots) => slots[strand_idx as usize],
            None => self.bands[self.strands[strand_idx as usize].band as usize].centre,
        }
    }
}

const fn range(range: &Range<u32>) -> Range<usize> {
    range.start as usize..range.end as usize
}
