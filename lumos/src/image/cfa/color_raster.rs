//! [`ColorRaster`]: the photosites of one color of a mosaic, in raster order.

use arrayvec::ArrayVec;

use crate::image::cfa::cfa_type::CfaType;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// The photosites of one color of a mosaic over an image, in raster order: how many there are,
/// and the pixel index of each by its rank among them, in closed form — the pattern repeats, so a
/// rank needs no walk over the pixels before it.
#[derive(Debug, Clone)]
pub(crate) struct ColorRaster {
    width: usize,
    period: usize,
    /// For each row phase, the columns of the color within one period, ascending.
    columns: ArrayVec<ArrayVec<u8, { CfaType::MAX_PERIOD }>, { CfaType::MAX_PERIOD }>,
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
        let columns: ArrayVec<ArrayVec<u8, { CfaType::MAX_PERIOD }>, { CfaType::MAX_PERIOD }> = (0
            ..period)
            .map(|row| {
                (0..period)
                    .filter(|&column| cfa_type.slot_at(Vec2us::new(column, row)) == color)
                    .map(|column| u8::try_from(column).expect("a column within the period"))
                    .collect()
            })
            .collect();
        let row_counts: ArrayVec<usize, { CfaType::MAX_PERIOD }> = columns
            .iter()
            .map(|columns| {
                let tail = size.width % period;
                (size.width / period) * columns.len()
                    + columns
                        .iter()
                        .filter(|&&column| usize::from(column) < tail)
                        .count()
            })
            .collect();
        let period_count = row_counts.iter().sum();
        let count = (size.height / period) * period_count
            + row_counts[..size.height % period].iter().sum::<usize>();
        Self {
            width: size.width,
            period,
            columns,
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
    pub(crate) fn index(&self, rank: usize) -> usize {
        debug_assert!(rank < self.count, "rank {rank} of {}", self.count);
        let mut row = rank / self.period_count * self.period;
        let mut rank = rank % self.period_count;
        let mut phase = 0;
        while rank >= self.row_counts[phase] {
            rank -= self.row_counts[phase];
            phase += 1;
        }
        row += phase;
        let columns = &self.columns[phase];
        let column =
            rank / columns.len() * self.period + usize::from(columns[rank % columns.len()]);
        row * self.width + column
    }
}

#[cfg(test)]
mod tests {
    use crate::image::cfa::cfa_pattern::CfaPattern;
    use crate::image::cfa::cfa_type::CfaType;
    use crate::image::cfa::color_raster::ColorRaster;
    use crate::image::cfa::xtrans_pattern::XTransPattern;
    use crate::math::size2us::Size2us;
    use crate::math::vec2us::Vec2us;

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
            CfaType::Bayer(CfaPattern::Rggb),
            CfaType::Bayer(CfaPattern::Gbrg),
            CfaType::XTrans(xtrans),
        ] {
            for (width, height) in [(12, 12), (13, 7), (7, 13), (1, 1), (5, 2)] {
                let size = Size2us::new(width, height);
                for color in 0..3 {
                    let walk: Vec<usize> = (0..size.pixel_count())
                        .filter(|&index| {
                            cfa.slot_at(Vec2us::new(index % width, index / width)) == color
                        })
                        .collect();
                    let raster = ColorRaster::new(&cfa, size, color);
                    assert_eq!(
                        raster.count(),
                        walk.len(),
                        "{cfa:?} {width}x{height} {color}"
                    );
                    for (rank, &index) in walk.iter().enumerate() {
                        assert_eq!(
                            raster.index(rank),
                            index,
                            "{cfa:?} {width}x{height} color {color} rank {rank}"
                        );
                    }
                }
            }
        }
    }
}
