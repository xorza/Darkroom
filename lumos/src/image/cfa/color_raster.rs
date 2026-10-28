//! [`ColorRaster`]: the photosites of one color of a mosaic, in raster order.

use arrayvec::ArrayVec;

use crate::image::cfa::cfa_type::CfaType;
use common::Size2us;
use glam::USizeVec2;

/// The photosites of one color of a mosaic over an image, in raster order: how many there are,
/// and the pixel index of each by its rank among them, in closed form — the pattern repeats, so a
/// rank needs no walk over the pixels before it.
#[derive(Debug, Clone)]
pub(crate) struct ColorRaster {
    width: usize,
    period: usize,
    /// For each row phase, the columns of the color within one period, ascending.
    cols: ArrayVec<ArrayVec<u8, { CfaType::MAX_PERIOD }>, { CfaType::MAX_PERIOD }>,
    /// For each row phase, the photosites of the color in one whole row.
    row_counts: ArrayVec<usize, { CfaType::MAX_PERIOD }>,
    /// The photosites of the color in one period of rows.
    period_count: usize,
    count: usize,
}

impl ColorRaster {
    /// The photosites of `color` of a `cfa_type` mosaic over an image of `size`.
    pub(crate) fn new(cfa_type: &CfaType, size: Size2us, color: usize) -> Self {
        let period = cfa_type.period();
        let cols: ArrayVec<ArrayVec<u8, { CfaType::MAX_PERIOD }>, { CfaType::MAX_PERIOD }> = (0
            ..period)
            .map(|row| {
                (0..period)
                    .filter(|&col| cfa_type.slot_at(USizeVec2::new(col, row)) == color)
                    .map(|col| u8::try_from(col).expect("a column within the period"))
                    .collect()
            })
            .collect();
        let row_counts: ArrayVec<usize, { CfaType::MAX_PERIOD }> = cols
            .iter()
            .map(|cols| {
                let tail = size.width % period;
                (size.width / period) * cols.len()
                    + cols.iter().filter(|&&col| usize::from(col) < tail).count()
            })
            .collect();
        let period_count = row_counts.iter().sum();
        let count = (size.height / period) * period_count
            + row_counts[..size.height % period].iter().sum::<usize>();
        Self {
            width: size.width,
            period,
            cols,
            row_counts,
            period_count,
            count,
        }
    }

    /// How many photosites of the color the image holds.
    pub(crate) const fn count(&self) -> usize {
        self.count
    }

    /// The pixel index of the photosite of rank `rank` among the color's, in raster order.
    pub(crate) fn idx(&self, rank: usize) -> usize {
        debug_assert!(rank < self.count, "rank {rank} of {}", self.count);
        let mut row = rank / self.period_count * self.period;
        let mut rank = rank % self.period_count;
        let mut phase = 0;
        while rank >= self.row_counts[phase] {
            rank -= self.row_counts[phase];
            phase += 1;
        }
        row += phase;
        let cols = &self.cols[phase];
        let col = rank / cols.len() * self.period + usize::from(cols[rank % cols.len()]);
        row * self.width + col
    }
}

#[cfg(test)]
mod tests {
    use crate::image::cfa::bayer_pattern::BayerPattern;
    use crate::image::cfa::cfa_type::CfaType;
    use crate::image::cfa::color_raster::ColorRaster;
    use crate::image::cfa::mosaic::Mosaic;
    use crate::image::cfa::xtrans_pattern::XTransPattern;
    use common::Size2us;
    use glam::USizeVec2;

    /// Every rank lands where a walk over the pixels in raster order finds that photosite, for each
    /// color of an RGGB and an X-Trans mosaic, over sizes that end mid-period on either axis or
    /// both, and a single pixel.
    #[test]
    fn a_rank_is_the_raster_walks_photosite() {
        let xtrans = XTransPattern::new([
            [1, 1, 0, 1, 1, 2],
            [1, 1, 2, 1, 1, 0],
            [2, 0, 1, 0, 2, 1],
            [1, 1, 2, 1, 1, 0],
            [1, 1, 0, 1, 1, 2],
            [0, 2, 1, 2, 0, 1],
        ])
        .unwrap();
        for cfa in [
            CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb)),
            CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Gbrg)),
            CfaType::Mosaic(Mosaic::XTrans(xtrans)),
        ] {
            for (width, height) in [(12, 12), (13, 7), (7, 13), (1, 1), (5, 2)] {
                let size = Size2us::new(width, height);
                for color in 0..3 {
                    let walk: Vec<usize> = (0..size.pixel_count())
                        .filter(|&idx| {
                            cfa.slot_at(USizeVec2::new(idx % width, idx / width)) == color
                        })
                        .collect();
                    let raster = ColorRaster::new(&cfa, size, color);
                    assert_eq!(
                        raster.count(),
                        walk.len(),
                        "{cfa:?} {width}x{height} {color}"
                    );
                    for (rank, &idx) in walk.iter().enumerate() {
                        assert_eq!(
                            raster.idx(rank),
                            idx,
                            "{cfa:?} {width}x{height} color {color} rank {rank}"
                        );
                    }
                }
            }
        }
    }
}
