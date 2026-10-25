//! [`BandScan`]: the input pixels whose drops can reach one band of the output, and the
//! [`Outline`] that bounds them.

use std::ops::Range;

use glam::DVec2;

use common::{Size2us, Vec2us};

/// Input rows of slack on the strip an outline is cut by: far above the rounding of an outline's
/// vertices — coordinates of 10⁵ pixels carry 10⁻¹¹ — and far below a pixel, so it scans no
/// column more, while a pixel centred exactly on a vertex is not lost to that rounding.
const STRIP_GUARD: f64 = 1e-6;

/// A band's widened output rectangle taken back onto the input.
#[derive(Debug, Clone, Copy)]
pub(super) enum Outline {
    /// A closed-form map's image, whose columns bound a scan.
    Corners(Corners),
    /// A sampled SIP image, of which only the rows bound a scan, as `FrameSource::band_scan`
    /// states: the least and the greatest input row it spans, widened by its reach.
    Rows([f64; 2]),
}

/// A closed-form map's image of a band's output rectangle: its four corners, and how far from the
/// outline they close, in input pixels, a pixel whose drop reaches inside can lie.
#[derive(Debug, Clone, Copy)]
pub(super) struct Corners {
    pub(super) vertices: [DVec2; 4],
    pub(super) reach: f64,
}

impl Outline {
    /// The least and greatest input row the outline spans, widened by its reach.
    fn rows(&self) -> [f64; 2] {
        match self {
            Self::Corners(corners) => corners.rows(),
            Self::Rows(rows) => *rows,
        }
    }
}

impl Corners {
    /// The least and greatest input row the outline spans, widened by its reach.
    fn rows(&self) -> [f64; 2] {
        let [first, last] = self.vertices.iter().fold(
            [f64::INFINITY, f64::NEG_INFINITY],
            |[first, last], vertex| [first.min(vertex.y), last.max(vertex.y)],
        );
        [first - self.reach, last + self.reach]
    }

    /// The input columns of row `row` within reach of the outline, on a frame `width` wide.
    ///
    /// The outline bounds a region, so the region's widest point within the strip of rows a drop
    /// reaches from `row` lies on the outline inside the strip, or where the outline crosses its
    /// edges: the edges cut to the strip give the extent, for any outline, convex or not.
    #[expect(
        clippy::cast_sign_loss,
        reason = "each bound is held at 0 or above first, and the cast saturates at the top"
    )]
    fn cols(&self, row: usize, width: usize) -> Range<usize> {
        let low = row as f64 - self.reach - STRIP_GUARD;
        let high = row as f64 + self.reach + STRIP_GUARD;
        let mut extent = [f64::INFINITY, f64::NEG_INFINITY];
        let edges = self
            .vertices
            .iter()
            .zip(self.vertices.iter().cycle().skip(1));
        for (&a, &b) in edges {
            if a.y.max(b.y) < low || a.y.min(b.y) > high {
                continue;
            }
            let [enter, leave] = if a.y == b.y {
                [0.0, 1.0]
            } else {
                let [to_low, to_high] = [low, high].map(|y| (y - a.y) / (b.y - a.y));
                [to_low.min(to_high).max(0.0), to_low.max(to_high).min(1.0)]
            };
            for t in [enter, leave] {
                let x = a.x + t * (b.x - a.x);
                extent = [extent[0].min(x), extent[1].max(x)];
            }
        }
        if extent[0] > extent[1] {
            return 0..0;
        }
        let start = ((extent[0] - self.reach).floor().max(0.0) as usize).min(width);
        let end = ((extent[1] + self.reach).ceil() + 1.0).max(0.0) as usize;
        start..end.min(width).max(start)
    }
}

/// The input pixels whose drops can reach one band of the output: a run of rows, and in each row
/// the columns its outline spans.
#[derive(Debug, Clone)]
pub(super) struct BandScan {
    rows: Range<usize>,
    /// `None` when nothing bounds the band's columns, and every column of a row is scanned.
    outline: Option<Corners>,
    width: usize,
}

impl BandScan {
    /// The scan of a band of a frame of `size` whose output rectangle came back onto the input as
    /// `outline`: every pixel when nothing bounds it, else the rows the outline spans and, where
    /// they bound a scan, its columns.
    #[expect(
        clippy::cast_sign_loss,
        reason = "each bound is held at 0 or above first, and the cast saturates at the top"
    )]
    pub(super) fn new(outline: Option<Outline>, size: Size2us) -> Self {
        let Size2us { width, height } = size;
        let Some(outline) = outline else {
            return Self {
                rows: 0..height,
                outline: None,
                width,
            };
        };
        // Saturating float casts, so a degenerate inverse (non-finite vertices) yields an empty
        // range rather than a wild one.
        let [first, last] = outline.rows();
        let start = (first.floor().max(0.0) as usize).min(height);
        let end = (last.ceil() + 1.0).max(0.0) as usize;
        Self {
            rows: start..end.min(height).max(start),
            outline: match outline {
                Outline::Corners(corners) => Some(corners),
                Outline::Rows(_) => None,
            },
            width,
        }
    }

    /// The input rows the band scans.
    pub(super) fn rows(&self) -> Range<usize> {
        self.rows.clone()
    }

    /// The input columns the band scans in `row`.
    pub(super) fn cols(&self, row: usize) -> Range<usize> {
        match &self.outline {
            Some(outline) => outline.cols(row, self.width),
            None => 0..self.width,
        }
    }

    /// Whether the band scans input pixel `pixel`.
    pub(super) fn contains(&self, pixel: Vec2us) -> bool {
        self.rows.contains(&pixel.y) && self.cols(pixel.y).contains(&pixel.x)
    }
}
