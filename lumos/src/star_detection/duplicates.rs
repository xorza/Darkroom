//! [`Duplicates`]: the search for detections of one star, over working sets it keeps from frame to
//! frame.

use crate::star_detection::star::Star;

/// The duplicate search, with its working sets kept from frame to frame: each star's cell beside
/// its index, and which stars it keeps.
#[derive(Debug, Default)]
pub(crate) struct Duplicates {
    members: Vec<Member>,
    kept: Vec<bool>,
}

/// A grid cell of side `min_separation`, row-major in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Cell {
    y: i64,
    x: i64,
}

/// A star in its cell; sorted, each cell's run is in input order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Member {
    cell: Cell,
    star: usize,
}

impl Duplicates {
    /// Remove duplicate star detections that are too close together.
    ///
    /// For each cluster of stars within `min_separation`, keeps the *first* star encountered in
    /// `stars` and drops the rest: a star is a duplicate of an earlier *kept* star strictly closer
    /// than `min_separation`, so a dropped star never suppresses a later one. Nothing here compares
    /// `.flux`, so callers MUST pass `stars` already sorted by flux descending (as
    /// `FilterOutcome::from_stars` does via `sort_by_flux`) for "first kept" to mean "brightest
    /// kept".
    ///
    /// One pass in input order over the stars sorted by their cell of side `min_separation`, each
    /// cell's run in input order: a star reads the earlier stars of its own and the eight
    /// neighbouring cells, found by binary search, so the cost is O(n log n) at any count and the
    /// memory one entry per star, where a dense grid would cost the field's area.
    ///
    /// Deliberately not `registration::kd_tree::KdTree`, which is the crate's other spatial index:
    /// its radius queries return the stars after a star as well as before it, and the pass needs
    /// only those before.
    pub(crate) fn remove(&mut self, stars: &mut Vec<Star>, min_separation: f32) -> usize {
        // A pair is a duplicate only when strictly closer than `min_separation`, so a separation of
        // zero removes nothing. Said here rather than reached: the cells would divide by it.
        if stars.len() < 2 || min_separation == 0.0 {
            return 0;
        }
        let min_sep_sq = f64::from(min_separation).powi(2);
        // A cell at least the separation wide is all the search needs; one pixel at the least keeps
        // a frame's coordinates over the cell side within an `i64`, where a separation of 1e-30
        // would carry them past it.
        let cell_size = f64::from(min_separation).max(1.0);
        let cell_of = |star: &Star| Cell {
            y: (star.pos.y / cell_size).floor() as i64,
            x: (star.pos.x / cell_size).floor() as i64,
        };
        let Self { members, kept } = self;
        members.clear();
        members.extend(stars.iter().enumerate().map(|(idx, star)| Member {
            cell: cell_of(star),
            star: idx,
        }));
        members.sort_unstable();
        kept.clear();
        kept.resize(stars.len(), true);
        for i in 0..stars.len() {
            let star = stars[i].pos;
            let home = cell_of(&stars[i]);
            let duplicate = (-1..=1).any(|dy| {
                (-1..=1).any(|dx| {
                    let cell = Cell {
                        y: home.y + dy,
                        x: home.x + dx,
                    };
                    let start = members.partition_point(|member| member.cell < cell);
                    members[start..]
                        .iter()
                        .take_while(|member| member.cell == cell && member.star < i)
                        .any(|member| {
                            kept[member.star]
                                && star.distance_squared(stars[member.star].pos) < min_sep_sq
                        })
                })
            });
            kept[i] = !duplicate;
        }

        let removed = kept.iter().filter(|&&keep| !keep).count();
        let mut idx = 0;
        stars.retain(|_| {
            let keep = kept[idx];
            idx += 1;
            keep
        });
        removed
    }
}
