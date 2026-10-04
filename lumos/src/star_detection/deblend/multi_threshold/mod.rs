//! Multi-threshold deblending, after SExtractor (Bertin & Arnouts 1996, A&AS 117, 393) as SEP
//! implements it in `deblend.c`.
//!
//! 1. Cut the component at `n_thresholds − 1` levels spaced exponentially between the detection
//!    threshold and its peak. At each level, every object of the level below splits into the
//!    connected regions of its pixels above the level that hold at least `min_area` pixels.
//! 2. An object is significant when its flux above its own level, `Σv − t·n`, exceeds
//!    `min_contrast` of the whole component's flux.
//! 3. From the top level down, an object with two or more significant sons splits: each son that
//!    is significant and split nowhere below becomes a star, and a split marks every ancestor as
//!    split. A component that split nowhere is one star.

use crate::star_detection::config::detection_config::Connectivity;
use crate::star_detection::deblend::component::Component;
use crate::star_detection::deblend::component_pixels::ComponentPixels;
use crate::star_detection::deblend::deblend_buffers::DeblendBuffers;
use crate::star_detection::deblend::region::Region;
use crate::star_detection::deblend::{Pixel, peaks_too_close};

/// A pixel that belongs to no object of the current level: below it, or in a region too small to
/// be one.
const NO_OBJECT: u32 = u32::MAX;

/// What the multi-threshold deblender reads from the detection configuration.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MultiThresholdParams {
    /// The levels of the ladder, the detection threshold included.
    pub(crate) n_thresholds: usize,
    /// A branch's share of the component's flux.
    pub(crate) min_contrast: f32,
    /// Minimum distance between the peaks of two stars of one component, in pixels.
    pub(crate) min_separation: usize,
    /// The fewest pixels a region above a level holds to be an object, as SEP's `minarea`.
    pub(crate) min_area: usize,
    /// How the pixels above a level join into regions — the labeling's own rule.
    pub(crate) connectivity: Connectivity,
}

impl MultiThresholdParams {
    /// Split `component` by its multi-threshold tree onto `out`: one region when it splits nowhere,
    /// else one region per star, brightest by flux above its level first. Returns how many it pushed.
    ///
    /// `floor` is the detection threshold at the component, in the units of its values — the lowest
    /// level of the ladder, as SExtractor's `DETECT_THRESH` is. It must be positive.
    pub(crate) fn deblend(
        self,
        component: &Component<'_>,
        floor: f32,
        buffers: &mut DeblendBuffers,
        out: &mut Vec<Region>,
    ) -> usize {
        let params = self;
        debug_assert!(floor > 0.0, "the ladder starts at a positive threshold");
        debug_assert!(params.n_thresholds >= 1, "the ladder has at least one step");
        let DeblendBuffers {
            pixels,
            peaks,
            assignment,
            tree,
            ..
        } = buffers;
        peaks.clear();
        let peak = component.peak();

        // Stars are disjoint subsets of the component, each holding more than `min_contrast` of its
        // flux above a positive level, so at most one can for `min_contrast ≥ 1`; and a peak at or
        // below the floor leaves the ladder no room to split.
        if params.min_contrast >= 1.0 || peak.value <= floor {
            return component.split_at(peaks, assignment, out);
        }

        pixels.fill(component);
        let ladder = ThresholdLadder {
            low: floor,
            high: peak.value,
            n_thresholds: params.n_thresholds,
        };
        tree.build(pixels, peak, ladder, params);
        let root_flux: f64 = pixels.pixels.iter().map(|p| f64::from(p.value)).sum();
        tree.find_stars((f64::from(params.min_contrast) * root_flux) as f32);

        let TreeBuffers { objects, stars, .. } = tree;
        if stars.len() > 1 {
            stars.sort_unstable_by(|&a, &b| {
                objects[b as usize]
                    .flux_above
                    .total_cmp(&objects[a as usize].flux_above)
                    .then(a.cmp(&b))
            });
            let min_sep_sq = params.min_separation * params.min_separation;
            for &star in stars.iter() {
                let candidate = objects[star as usize].peak;
                if !peaks
                    .iter()
                    .any(|kept: &Pixel| peaks_too_close(candidate.pos, kept.pos, min_sep_sq))
                {
                    peaks.push(candidate);
                }
            }
        }
        component.split_at(peaks, assignment, out)
    }
}

/// One object of the tree: a connected region of the component above the level it was cut at.
#[derive(Debug, Clone, Copy)]
struct TreeObject {
    /// The object of the level below it lies in; the root's is its own index.
    parent: u32,
    peak: Pixel,
    /// `Σv − t·n` over its pixels: the flux it holds above its level.
    flux_above: f32,
}

/// The multi-threshold deblender's working sets, reused from component to component. Every one
/// is in proportion to the component's pixels or to its tree.
#[derive(Debug, Default)]
pub(crate) struct TreeBuffers {
    /// Per pixel: the object of the current level it lies in, or [`NO_OBJECT`].
    object_of: Vec<u32>,
    /// Per pixel: the level it was last reached at.
    visited: Vec<u32>,
    /// The region being grown, as pixel indices.
    members: Vec<u32>,
    queue: Vec<u32>,
    objects: Vec<TreeObject>,
    /// The sons of each object, by object, delimited by `son_starts`.
    sons: Vec<u32>,
    son_starts: Vec<u32>,
    /// Per object: no object under it split.
    unsplit: Vec<bool>,
    /// The objects that stand as stars.
    stars: Vec<u32>,
}

/// The exponentially spaced ladder one component is cut at: the floor to start from, the peak to
/// reach, and how many steps in between. The three are only meaningful together — level `i` sits
/// at `low * (high / low) ^ (i / n)`.
#[derive(Debug, Clone, Copy)]
struct ThresholdLadder {
    low: f32,
    high: f32,
    n_thresholds: usize,
}

impl ThresholdLadder {
    /// Level `i` of `0..n_thresholds`: `low` itself at 0.
    fn level(self, i: usize) -> f32 {
        self.low * (self.high / self.low).powf(i as f32 / self.n_thresholds as f32)
    }
}

impl TreeBuffers {
    /// Build the tree of `pixels`, whose brightest is `peak`: the root is the whole component,
    /// and levels `1..n_thresholds` cut every object of the level below into its regions above
    /// the level holding at least `min_area` pixels. Exponential spacing puts the levels densest at
    /// the faint end, where neighbours first separate.
    fn build(
        &mut self,
        pixels: &ComponentPixels,
        peak: Pixel,
        ladder: ThresholdLadder,
        params: MultiThresholdParams,
    ) {
        let count = pixels.pixels.len();
        self.object_of.clear();
        self.object_of.resize(count, 0);
        self.visited.clear();
        self.visited.resize(count, 0);
        self.objects.clear();
        self.objects.push(TreeObject {
            parent: 0,
            peak,
            flux_above: 0.0,
        });

        for level in 1..ladder.n_thresholds {
            let threshold = ladder.level(level);
            let stamp = u32::try_from(level).expect("the ladder holds at most u32 levels");
            let before = self.objects.len();
            for seed in 0..count {
                let parent = self.object_of[seed];
                if pixels.pixels[seed].value < threshold
                    || parent == NO_OBJECT
                    || self.visited[seed] == stamp
                {
                    continue;
                }
                self.grow(pixels, seed, threshold, stamp, params.connectivity);
                let sum: f64 = self
                    .members
                    .iter()
                    .map(|&member| f64::from(pixels.pixels[member as usize].value))
                    .sum();
                let object = if self.members.len() < params.min_area {
                    NO_OBJECT
                } else {
                    let peak = Pixel::brightest(
                        self.members
                            .iter()
                            .map(|&member| pixels.pixels[member as usize]),
                    )
                    .expect("a region holds its seed");
                    self.objects.push(TreeObject {
                        parent,
                        peak,
                        flux_above: (sum - f64::from(threshold) * self.members.len() as f64) as f32,
                    });
                    u32::try_from(self.objects.len() - 1).expect("a tree holds below u32 objects")
                };
                for &member in &self.members {
                    self.object_of[member as usize] = object;
                }
            }
            if self.objects.len() == before {
                break;
            }
        }
    }

    /// Into `members`, the connected pixels at or above `threshold` reached from `seed`, each
    /// marked visited at `stamp`. The pixels above a level nest inside the regions of the level
    /// below, so all of them lie in the seed's object.
    fn grow(
        &mut self,
        pixels: &ComponentPixels,
        seed: usize,
        threshold: f32,
        stamp: u32,
        connectivity: Connectivity,
    ) {
        let Self {
            visited,
            members,
            queue,
            ..
        } = self;
        members.clear();
        queue.clear();
        visited[seed] = stamp;
        queue.push(seed as u32);
        while let Some(index) = queue.pop() {
            members.push(index);
            pixels.for_each_neighbour(index as usize, connectivity, |neighbour| {
                if visited[neighbour] != stamp && pixels.pixels[neighbour].value >= threshold {
                    visited[neighbour] = stamp;
                    queue.push(neighbour as u32);
                }
            });
        }
    }

    /// Into `stars`, the objects that stand as stars when a significant one holds more than
    /// `min_flux` above its level.
    ///
    /// The objects are visited from the last cut down, so every object's sons are decided before
    /// it is. An object with two or more significant sons splits, and each of them that is
    /// unsplit is a star. As in SEP, the bar is one value for the component, not a share of the
    /// parent: a parent-relative bar over-splits the bright wings of large stars.
    fn find_stars(&mut self, min_flux: f32) {
        let Self {
            objects,
            sons,
            son_starts,
            unsplit,
            stars,
            ..
        } = self;
        let count = objects.len();
        stars.clear();
        if count < 3 {
            return;
        }
        // The sons of every object, grouped by parent in index order: a count per parent, a
        // running sum to each parent's start, a placement that advances each start to its end,
        // and a shift that turns the ends back into starts.
        son_starts.clear();
        son_starts.resize(count + 1, 0);
        for object in &objects[1..] {
            son_starts[object.parent as usize + 1] += 1;
        }
        for index in 0..count {
            son_starts[index + 1] += son_starts[index];
        }
        sons.clear();
        sons.resize(count - 1, 0);
        for (index, object) in objects.iter().enumerate().skip(1) {
            let slot = &mut son_starts[object.parent as usize];
            sons[*slot as usize] = index as u32;
            *slot += 1;
        }
        son_starts.copy_within(0..count, 1);
        son_starts[0] = 0;

        let significant = |object: u32| objects[object as usize].flux_above > min_flux;
        unsplit.clear();
        unsplit.resize(count, true);
        for index in (0..count).rev() {
            let own = &sons[son_starts[index] as usize..son_starts[index + 1] as usize];
            unsplit[index] = own.iter().all(|&son| unsplit[son as usize]);
            if own.iter().filter(|&&son| significant(son)).count() > 1 {
                stars.extend(
                    own.iter()
                        .copied()
                        .filter(|&son| unsplit[son as usize] && significant(son)),
                );
                unsplit[index] = false;
            }
        }
        if unsplit[0] {
            stars.clear();
        }
    }
}

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
