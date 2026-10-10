//! [`ColourMesh`]: the local sky and σ of each colour of a mosaic, between tile centres.

use arrayvec::ArrayVec;
use imaginarium::Buffer2;

use crate::background_mesh::tile_stats::TileStats;
use crate::background_mesh::workspace::MeshWorkspace;
use crate::bit_buffer2::BitBuffer2;
use crate::io::image::cfa::CfaType;
use crate::math::vec2us::Vec2us;

/// The σ-clip passes of each tile, as the star detection's background takes by default.
const SIGMA_CLIP_ITERATIONS: usize = 3;

/// A tile mesh per colour of a mosaic, one for a mono plane, read bilinearly between the tile
/// centres.
///
/// Each colour's tiles measure that colour's photosites alone, so a flat-fielded frame whose colours
/// sit at different levels gives each its own sky, and a colour's σ is not inflated by the others'
/// offsets. The 3×3 median over the tiles keeps a tile that a cluster of hot pixels or a bright
/// object spoiled from reaching its neighbours.
#[derive(Debug)]
pub(crate) struct ColourMesh {
    colours: ArrayVec<Buffer2<TileStats>, 3>,
    x: Vec<Span>,
    y: Vec<Span>,
}

/// The background at one pixel of one colour: the sky, and the noise about the tiles' planes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LocalBackground {
    pub(crate) sky: f32,
    pub(crate) noise: f32,
}

/// Where one coordinate falls between two tile centres.
#[derive(Debug, Clone, Copy)]
struct Span {
    lower: usize,
    upper: usize,
    fraction: f32,
}

impl ColourMesh {
    /// The mesh of `pixels` under `cfa`, in tiles of `tile_size`, leaving out the pixels `excluded`
    /// holds.
    pub(crate) fn measure(
        pixels: &Buffer2<f32>,
        cfa: &CfaType,
        tile_size: usize,
        excluded: Option<&BitBuffer2>,
        workspace: &mut MeshWorkspace,
    ) -> Self {
        let mut colours = ArrayVec::new();
        let mut x = Vec::new();
        let mut y = Vec::new();
        for colour in 0..cfa.num_colors() {
            let grid = if *cfa == CfaType::Mono {
                workspace.tile_stats(pixels, excluded, tile_size, SIGMA_CLIP_ITERATIONS, true)
            } else {
                let colour = u8::try_from(colour).expect("three colours");
                workspace.tile_stats_where(
                    pixels,
                    &|position| {
                        cfa.color_at(position) == colour
                            && excluded.is_none_or(|excluded| !excluded.get_at(position))
                    },
                    tile_size,
                    SIGMA_CLIP_ITERATIONS,
                    true,
                )
            };
            colours.push(grid.stats.clone());
            // Every colour's grid has the one layout of the frame.
            if x.is_empty() {
                x = spans(pixels.width(), &grid.centers_x);
                y = spans(pixels.height(), &grid.centers_y);
            }
        }
        Self { colours, x, y }
    }

    /// The sky and noise of `colour` at `position`, bilinear between the four nearest tile centres,
    /// and continued linearly past the outer ones.
    pub(crate) fn at(&self, colour: usize, position: Vec2us) -> LocalBackground {
        let tiles = &self.colours[colour];
        let x = self.x[position.x];
        let y = self.y[position.y];
        let corner = |tx: usize, ty: usize| tiles[(tx, ty)];
        let lerp = |a: f32, b: f32, t: f32| a + t * (b - a);
        let mix = |get: fn(TileStats) -> f32| {
            let top = lerp(
                get(corner(x.lower, y.lower)),
                get(corner(x.upper, y.lower)),
                x.fraction,
            );
            let bottom = lerp(
                get(corner(x.lower, y.upper)),
                get(corner(x.upper, y.upper)),
                x.fraction,
            );
            lerp(top, bottom, y.fraction)
        };
        LocalBackground {
            sky: mix(|tile| tile.sky),
            // The linear continuation can carry σ below zero past a steep edge, which no noise is.
            noise: mix(|tile| tile.noise).max(0.0),
        }
    }
}

/// For every coordinate of an axis of `length`, the tile centres it falls between.
fn spans(length: usize, centres: &[f32]) -> Vec<Span> {
    if centres.len() == 1 {
        return vec![
            Span {
                lower: 0,
                upper: 0,
                fraction: 0.0,
            };
            length
        ];
    }
    (0..length)
        .map(|position| {
            let position = position as f32;
            let upper = centres
                .partition_point(|&centre| centre <= position)
                .clamp(1, centres.len() - 1);
            let lower = upper - 1;
            Span {
                lower,
                upper,
                fraction: (position - centres[lower]) / (centres[upper] - centres[lower]),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::raw::demosaic::bayer::CfaPattern;

    /// An RGGB mosaic of 128 × 128 with red at 1/8 + x/1024, green at 1/4 and blue at 3/8: each
    /// colour's mesh reads its own level, and red's gradient. Tiles of 64 are centred at 31.5 and
    /// 95.5. A tile's sky is read about the plane fitted to its samples, so red's tiles read the
    /// ramp at their centres, and the bilinear reading between and past the centres follows it to
    /// any column: red(31) and red(63). Every value is dyadic.
    #[test]
    fn each_colour_reads_its_own_level() {
        let size = 128;
        let cfa = CfaType::Bayer(CfaPattern::Rggb);
        let red = |x: usize| 0.125 + x as f32 / 1024.0;
        let pixels = Buffer2::new(
            size,
            size,
            (0..size * size)
                .map(|index| {
                    let position = Vec2us::new(index % size, index / size);
                    match cfa.color_at(position) {
                        0 => red(position.x),
                        1 => 0.25,
                        _ => 0.375,
                    }
                })
                .collect(),
        );
        let mesh = ColourMesh::measure(&pixels, &cfa, 64, None, &mut MeshWorkspace::default());
        for position in [
            Vec2us::new(31, 31),
            Vec2us::new(64, 10),
            Vec2us::new(100, 90),
        ] {
            assert_eq!(mesh.at(1, position).sky, 0.25, "{position:?}");
            assert_eq!(mesh.at(2, position).sky, 0.375, "{position:?}");
            assert_eq!(mesh.at(1, position).noise, 0.0, "{position:?}");
        }
        assert_eq!(mesh.at(0, Vec2us::new(31, 31)).sky, red(31));
        assert_eq!(mesh.at(0, Vec2us::new(63, 31)).sky, red(63));
    }
}
