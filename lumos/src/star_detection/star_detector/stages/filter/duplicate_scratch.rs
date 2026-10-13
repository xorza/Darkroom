//! [`DuplicateScratch`]: the duplicate search, over working sets it keeps from frame to frame.

use crate::star_detection::star::Star;

/// The duplicate search's working sets, kept from frame to frame: each star's cell beside its
/// index, and which stars it keeps.
#[derive(Debug, Default)]
pub(crate) struct DuplicateScratch {
    members: Vec<((i64, i64), usize)>,
    kept: Vec<bool>,
}

impl DuplicateScratch {
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
    pub(crate) fn remove_duplicates(
        &mut self,
        stars: &mut Vec<Star>,
        min_separation: f32,
    ) -> usize {
        // A pair is a duplicate only when strictly closer than `min_separation`, so a separation of
        // zero removes nothing. Said here rather than reached: the cells would divide by it.
        if stars.len() < 2 || min_separation == 0.0 {
            return 0;
        }
        let min_sep_sq = f64::from(min_separation * min_separation);
        let cell_size = f64::from(min_separation);
        let cell_of = |star: &Star| {
            (
                (star.pos.y / cell_size).floor() as i64,
                (star.pos.x / cell_size).floor() as i64,
            )
        };
        let Self { members, kept } = self;
        members.clear();
        members.extend(
            stars
                .iter()
                .enumerate()
                .map(|(index, star)| (cell_of(star), index)),
        );
        members.sort_unstable();
        kept.clear();
        kept.resize(stars.len(), true);
        for i in 0..stars.len() {
            let star = stars[i].pos;
            let (cell_y, cell_x) = cell_of(&stars[i]);
            let duplicate = (-1..=1).any(|dy| {
                (-1..=1).any(|dx| {
                    let cell = (cell_y + dy, cell_x + dx);
                    let start = members.partition_point(|&(member, _)| member < cell);
                    members[start..]
                        .iter()
                        .take_while(|&&(member, j)| member == cell && j < i)
                        .any(|&(_, j)| kept[j] && star.distance_squared(stars[j].pos) < min_sep_sq)
                })
            });
            kept[i] = !duplicate;
        }

        let removed = kept.iter().filter(|&&keep| !keep).count();
        let mut index = 0;
        stars.retain(|_| {
            let keep = kept[index];
            index += 1;
            keep
        });
        removed
    }
}
