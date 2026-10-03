//! Tests for connected component labeling.
//!
//! Every mask runs through [`check`]: both connectivities, and every band count in
//! [`STRIP_COUNTS`] the height allows, each held to a flood-fill reference label for label.
//! Components are numbered in raster order of their first pixel whatever the bands, and the
//! reference seeds its fills in raster order, so the two label maps must be equal outright — which
//! also makes every band count agree with every other — and the boxes and areas collected from the
//! runs must equal a scan of the labels.

#![expect(
    clippy::cast_sign_loss,
    reason = "test fixtures are small images, with non-negative coordinates and offsets of a few dozen pixels"
)]

use std::collections::VecDeque;

use crate::bit_buffer2::BitBuffer2;
use crate::internals::prelude::*;
use crate::star_detection::config::detection_config::Connectivity;
use crate::star_detection::labeling::LabelMap;
use crate::star_detection::labeling::labeler::internals::label_in_strips;

mod parallel;
mod property_based;
mod runs;
mod shapes;

/// Band counts every mask is labelled in: one band, and several, so a mask more than a few rows
/// tall crosses band boundaries whatever the machine's thread count.
const STRIP_COUNTS: [usize; 5] = [1, 2, 3, 5, 8];

/// A binary mask with its size.
#[derive(Debug)]
struct Mask {
    size: Size2us,
    data: Vec<bool>,
}

impl Mask {
    /// One string per row, `#` set and `.` clear.
    fn ascii(rows: &[&str]) -> Self {
        let width = rows.first().map_or(0, |row| row.len());
        assert!(rows.iter().all(|row| row.len() == width), "ragged mask");
        let data = rows
            .iter()
            .flat_map(|row| row.bytes().map(|b| b == b'#'))
            .collect();
        Self {
            size: Size2us::new(width, rows.len()),
            data,
        }
    }

    /// The pixels `(x, y)` for which `set` holds.
    fn from_fn(size: Size2us, mut set: impl FnMut(usize, usize) -> bool) -> Self {
        let mut data = Vec::with_capacity(size.pixel_count());
        for y in 0..size.height {
            for x in 0..size.width {
                data.push(set(x, y));
            }
        }
        Self { size, data }
    }

    fn bits(&self) -> BitBuffer2 {
        BitBuffer2::from_slice(self.size, &self.data)
    }
}

/// The 4- and 8-connected labelings of a mask, as the production path computes them.
#[derive(Debug)]
struct Labelings {
    four: LabelMap,
    eight: LabelMap,
}

fn neighbours(connectivity: Connectivity) -> &'static [(isize, isize)] {
    match connectivity {
        Connectivity::Four => &[(0, -1), (-1, 0), (1, 0), (0, 1)],
        Connectivity::Eight => &[
            (-1, -1),
            (0, -1),
            (1, -1),
            (-1, 0),
            (1, 0),
            (-1, 1),
            (0, 1),
            (1, 1),
        ],
    }
}

/// Flood fill from each unlabelled foreground pixel in raster order: naive, slow and obviously
/// correct, and numbered the way the labeler numbers.
fn reference(mask: &Mask, connectivity: Connectivity) -> Vec<u32> {
    let Size2us { width, height } = mask.size;
    let mut labels = vec![0u32; mask.data.len()];
    let mut next = 0u32;
    let mut queue = VecDeque::new();
    for start in 0..mask.data.len() {
        if !mask.data[start] || labels[start] != 0 {
            continue;
        }
        next += 1;
        labels[start] = next;
        queue.push_back(start);
        while let Some(index) = queue.pop_front() {
            let (x, y) = ((index % width) as isize, (index / width) as isize);
            for &(dx, dy) in neighbours(connectivity) {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= width as isize || ny >= height as isize {
                    continue;
                }
                let neighbour = ny as usize * width + nx as usize;
                if mask.data[neighbour] && labels[neighbour] == 0 {
                    labels[neighbour] = next;
                    queue.push_back(neighbour);
                }
            }
        }
    }
    labels
}

/// The box and area the labeling collected from its runs against a scan of every labelled pixel.
fn verify_components(label_map: &LabelMap) {
    let size = Size2us::new(label_map.width(), label_map.height());
    let scanned = LabelMap::from_raw(
        Buffer2::new(size.width, size.height, label_map.labels().to_vec()),
        label_map.num_labels(),
    );
    assert_eq!(label_map.components().len(), scanned.components().len());
    for (from_runs, from_scan) in label_map.components().iter().zip(scanned.components()) {
        assert_eq!(
            (from_runs.label, from_runs.area, from_runs.bbox),
            (from_scan.label, from_scan.area, from_scan.bbox)
        );
    }
}

/// Label `mask` both ways, at every band count, against [`reference`]; see the module docs.
fn check(mask: &Mask) -> Labelings {
    let bits = mask.bits();
    let label = |connectivity: Connectivity| {
        let expected = reference(mask, connectivity);
        let count = expected.iter().copied().max().unwrap_or(0) as usize;
        let strips = STRIP_COUNTS
            .iter()
            .copied()
            .filter(|&strips| strips <= mask.size.height);
        for strips in strips {
            let map = label_in_strips(&bits, connectivity, strips);
            assert_eq!(
                map.labels(),
                expected,
                "{connectivity:?} in {strips} bands differs from the reference"
            );
            assert_eq!(map.num_labels(), count);
            verify_components(&map);
        }
        let map = LabelMap::from_mask(&bits, connectivity);
        assert_eq!(
            map.labels(),
            expected,
            "{connectivity:?} at the default bands"
        );
        verify_components(&map);
        map
    };
    Labelings {
        four: label(Connectivity::Four),
        eight: label(Connectivity::Eight),
    }
}
