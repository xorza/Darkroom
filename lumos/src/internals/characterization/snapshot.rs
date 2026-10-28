//! [`Snapshot`]: what a stage produced, compared with its reference file within a tolerance.

use std::fmt::Write as _;
use std::path::PathBuf;

use common::{Buffer2, SerdeFormat, env, fs};
use serde::{Deserialize, Serialize};

/// The cells a plane's side is summed in, at most: 256 sums keep a plane's reference short and
/// still name where in the plane a change landed.
const GRID: usize = 16;

/// How far a value may move from its reference, relative to the larger magnitude: 8 `f32` ulps,
/// 2⁻²⁰. Rust leaves the precision of `exp`, `ln`, `powf` and the other transcendental functions
/// to the platform's C library, which rounds them an ulp apart from another, and an `f32` stage
/// carries that through a few operations. Eight ulps covers it, as `common::FloatExt` allows, and
/// stays far below a change of the algorithm. A value from `f64` math on `f32` pixels moves as
/// those pixels do, so `f64` values take the same bound.
const RELATIVE: f64 = 0.000_000_953_674_316_406_25;
/// The same bound at 1, the scale of a normalized pixel, for values near zero, whose relative
/// difference says nothing.
const ABSOLUTE: f64 = RELATIVE;

/// The variable that, set to `update`, makes a check that fails write its reference anew.
const UPDATE: &str = "LUMOS_SNAPSHOTS";

/// The deviations a failure lists before it counts the rest.
const LISTED: usize = 8;

/// What one stage produced, entry by entry: each plane summed in cells, bytes by their digest,
/// every other value whole.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Snapshot(Vec<Entry>);

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    label: String,
    value: Value,
}

#[derive(Debug, Serialize, Deserialize)]
enum Value {
    Plane(Plane),
    F32s(Vec<f32>),
    F64s(Vec<f64>),
    Count(u64),
    /// The blake3 digest of bytes too many to keep: a full frame's flag plane, two hex digits a
    /// pixel, is 46.4 MiB at 24 MP. Skia Gold keeps an image by its digest the same way.
    Digest(#[serde(with = "common::hex_bytes")] [u8; 32]),
}

/// A plane by its size, its extremes and two `f64` sums of each cell of a grid of at most
/// [`GRID`] × [`GRID`], row by row: the plain sum, and the sum weighted by [`weight`], which moves
/// when two pixels of a cell trade places, as a sum alone does not.
#[derive(Debug, Serialize, Deserialize)]
struct Plane {
    width: usize,
    height: usize,
    min: f32,
    max: f32,
    cells: Vec<f64>,
    weighted: Vec<f64>,
}

impl Snapshot {
    pub(crate) fn plane(&mut self, label: &str, plane: &Buffer2<f32>) -> &mut Self {
        self.push(label, Value::Plane(Plane::of(plane)))
    }

    pub(crate) fn f32s(&mut self, label: &str, values: &[f32]) -> &mut Self {
        self.push(label, Value::F32s(values.to_vec()))
    }

    pub(crate) fn f64s(&mut self, label: &str, values: &[f64]) -> &mut Self {
        self.push(label, Value::F64s(values.to_vec()))
    }

    pub(crate) fn count(&mut self, label: &str, count: usize) -> &mut Self {
        self.push(label, Value::Count(count as u64))
    }

    pub(crate) fn digest(&mut self, label: &str, bytes: &[u8]) -> &mut Self {
        self.push(label, Value::Digest(blake3::hash(bytes).into()))
    }

    fn push(&mut self, label: &str, value: Value) -> &mut Self {
        self.0.push(Entry {
            label: label.to_owned(),
            value,
        });
        self
    }

    /// Compare with the reference `snapshots/<stage>.ron`: sizes, counts and bytes exactly, every
    /// other value within [`ABSOLUTE`] plus [`RELATIVE`] of the larger magnitude, as numpy's
    /// `isclose` bounds it. With `LUMOS_SNAPSHOTS=update`, a reference that differs, is missing
    /// or does not parse is written anew instead of failing.
    ///
    /// # Panics
    ///
    /// When the stage moved past the tolerance, or its reference is missing or unreadable.
    pub(crate) fn check(&self, stage: &str) {
        let path = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/snapshots"))
            .join(format!("{stage}.ron"));
        let differences = match fs::read(&path) {
            Ok(bytes) => match SerdeFormat::Ron.deserialize::<Self>(&bytes) {
                Ok(reference) => self.differences(&reference),
                Err(error) => vec![format!(
                    "the reference {} does not parse: {error}",
                    path.display()
                )],
            },
            Err(error) => vec![format!("no reference at {}: {error}", path.display())],
        };
        if differences.is_empty() {
            return;
        }
        if env::var_os(UPDATE).is_some_and(|mode| mode == "update") {
            fs::write(&path, SerdeFormat::Ron.serialize(self).unwrap()).unwrap();
            println!("UPDATED: the {stage} reference, {}", path.display());
            return;
        }
        let count = differences.len();
        let mut listed = differences[..count.min(LISTED)].join("\n  ");
        if count > LISTED {
            write!(listed, "\n  and {} more", count - LISTED).unwrap();
        }
        panic!(
            "the {stage} snapshot moved:\n  {listed}\nState the reason with the change, or find \
             the regression; LUMOS_SNAPSHOTS=update writes the new reference."
        );
    }

    /// One line for each entry or value that is not the reference's, in order.
    fn differences(&self, reference: &Self) -> Vec<String> {
        if self.0.len() != reference.0.len() {
            return vec![format!(
                "{} entries, the reference holds {}",
                self.0.len(),
                reference.0.len()
            )];
        }
        let mut differences = Vec::new();
        for (entry, expected) in self.0.iter().zip(&reference.0) {
            let label = &entry.label;
            if *label != expected.label {
                differences.push(format!("{label}, the reference holds {}", expected.label));
                continue;
            }
            match (&entry.value, &expected.value) {
                (Value::Plane(plane), Value::Plane(want)) => {
                    plane.differences(label, want, &mut differences);
                }
                (Value::F32s(values), Value::F32s(want)) => compare(
                    label,
                    values.iter().map(|&v| f64::from(v)),
                    want.iter().map(|&v| f64::from(v)),
                    |idx| format!("{label} {idx}"),
                    &mut differences,
                ),
                (Value::F64s(values), Value::F64s(want)) => compare(
                    label,
                    values.iter().copied(),
                    want.iter().copied(),
                    |idx| format!("{label} {idx}"),
                    &mut differences,
                ),
                (Value::Count(count), Value::Count(want)) => {
                    if count != want {
                        differences.push(format!("{label} is {count}, the reference {want}"));
                    }
                }
                (Value::Digest(digest), Value::Digest(want)) => {
                    if digest != want {
                        differences.push(format!("{label}: the digest is not the reference's"));
                    }
                }
                (value, want) => differences.push(format!(
                    "{label} is {}, the reference {}",
                    value.kind(),
                    want.kind()
                )),
            }
        }
        differences
    }
}

impl Value {
    const fn kind(&self) -> &'static str {
        match self {
            Self::Plane(_) => "a plane",
            Self::F32s(_) => "f32 values",
            Self::F64s(_) => "f64 values",
            Self::Count(_) => "a count",
            Self::Digest(_) => "a digest",
        }
    }
}

impl Plane {
    fn of(plane: &Buffer2<f32>) -> Self {
        let (width, height) = (plane.width(), plane.height());
        let (cell_width, cell_height) = (cell_side(width), cell_side(height));
        let cols = width.div_ceil(cell_width);
        let mut cells = vec![0.0; cols * height.div_ceil(cell_height)];
        let mut weighted = cells.clone();
        let (mut min, mut max) = (f32::INFINITY, f32::NEG_INFINITY);
        for (y, row) in plane.pixels().chunks_exact(width.max(1)).enumerate() {
            for (x, &pixel) in row.iter().enumerate() {
                let cell = y / cell_height * cols + x / cell_width;
                cells[cell] += f64::from(pixel);
                weighted[cell] += weight(y * width + x) * f64::from(pixel);
                min = min.min(pixel);
                max = max.max(pixel);
            }
        }
        Self {
            width,
            height,
            min,
            max,
            cells,
            weighted,
        }
    }

    fn differences(&self, label: &str, want: &Self, differences: &mut Vec<String>) {
        if (self.width, self.height) != (want.width, want.height) {
            differences.push(format!(
                "{label} is {} × {}, the reference {} × {}",
                self.width, self.height, want.width, want.height
            ));
            return;
        }
        compare(
            label,
            [self.min, self.max].map(f64::from).into_iter(),
            [want.min, want.max].map(f64::from).into_iter(),
            |idx| format!("{label} {}", ["min", "max"][idx]),
            differences,
        );
        let cols = self.width.div_ceil(cell_side(self.width));
        compare(
            label,
            self.cells.iter().copied(),
            want.cells.iter().copied(),
            |idx| format!("{label} cell ({}, {})", idx % cols, idx / cols),
            differences,
        );
        compare(
            label,
            self.weighted.iter().copied(),
            want.weighted.iter().copied(),
            |idx| format!("{label} weighted cell ({}, {})", idx % cols, idx / cols),
            differences,
        );
    }
}

/// The weight of pixel `idx` in a cell's weighted sum, in `[1, 2)`: the top 52 bits of `idx` times
/// 2⁶⁴/φ, Knuth's multiplicative hash, so neighbours take weights far apart. Integer arithmetic and
/// an exact conversion, the same on every host; positive, so the weighted sum takes the plain sum's
/// tolerance.
fn weight(idx: usize) -> f64 {
    let hash = (idx as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    1.0 + (hash >> 12) as f64 / (1u64 << 52) as f64
}

/// The side of a cell that cuts `side` pixels into at most [`GRID`] cells.
const fn cell_side(side: usize) -> usize {
    if side == 0 { 1 } else { side.div_ceil(GRID) }
}

/// A line in `differences` for each value of `got` past the tolerance from its own in `want`, as
/// `name` names its index, or one line for `label` when the counts differ. Two NaNs agree, as the
/// same computation gives both.
fn compare(
    label: &str,
    got: impl ExactSizeIterator<Item = f64>,
    want: impl ExactSizeIterator<Item = f64>,
    name: impl Fn(usize) -> String,
    differences: &mut Vec<String>,
) {
    if got.len() != want.len() {
        differences.push(format!(
            "{label}: {} values, the reference holds {}",
            got.len(),
            want.len()
        ));
        return;
    }
    for (idx, (got, want)) in got.zip(want).enumerate() {
        let off = (got - want).abs();
        let agree = got == want
            || (got.is_nan() && want.is_nan())
            || off <= RELATIVE * got.abs().max(want.abs()) + ABSOLUTE;
        if !agree {
            differences.push(format!(
                "{} is {got:e}, the reference {want:e}: {off:.2e} off",
                name(idx)
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use common::Buffer2;

    use crate::internals::characterization::snapshot::{Plane, Snapshot, weight};

    /// A 40 × 3 plane of `x + 100y`: cells of ⌈40/16⌉ = 3 pixels by ⌈3/16⌉ = 1 row, 14 to a row,
    /// the last one pixel wide. Cell (0, 0) holds 0 + 1 + 2 = 3, cell (13, 0) holds 39 alone, and
    /// cell (0, 2) holds 200 + 201 + 202 = 603.
    #[test]
    fn a_plane_sums_in_cells_of_the_grid() {
        let pixels = (0..3)
            .flat_map(|y| (0..40).map(move |x| (x + 100 * y) as f32))
            .collect();
        let plane = Plane::of(&Buffer2::new((40, 3), pixels));
        assert_eq!((plane.width, plane.height), (40, 3));
        assert_eq!((plane.min, plane.max), (0.0, 239.0));
        assert_eq!(plane.cells.len(), 14 * 3);
        assert_eq!(
            [plane.cells[0], plane.cells[13], plane.cells[28]],
            [3.0, 39.0, 603.0]
        );
        // Pixel 39 alone in cell (13, 0); pixels 1 and 2 beside 0 in cell (0, 0).
        assert_eq!(plane.weighted[13], 39.0 * weight(39));
        assert_eq!(plane.weighted[0], weight(1) + 2.0 * weight(2));
    }

    /// One `f32` ulp at 1, 2⁻²³, and 1e-7 next to zero are within the tolerance; 2⁻¹⁸ at 1, past
    /// 2⁻²⁰ + 2⁻²⁰ = 2⁻¹⁹, and 1e-5 next to zero are not, and each is named by its index or
    /// cell. Two NaNs agree; a count, a label or a size that differs is named whole.
    #[test]
    fn values_agree_within_the_tolerance_and_a_difference_is_named() {
        let values = |values: &[f32]| {
            let mut snapshot = Snapshot::default();
            snapshot.f32s("v", values);
            snapshot
        };
        let reference = values(&[1.0, 0.0, f32::NAN]);
        let ulp = 1.0 + f32::EPSILON;
        assert!(
            values(&[ulp, 1e-7, f32::NAN])
                .differences(&reference)
                .is_empty()
        );
        assert_eq!(
            values(&[1.0 + 2f32.powi(-18), 1e-5, f32::NAN]).differences(&reference),
            [
                "v 0 is 1.0000038146972656e0, the reference 1e0: 3.81e-6 off",
                "v 1 is 9.999999747378752e-6, the reference 0e0: 1.00e-5 off",
            ]
        );

        // Pixels 0 and 1 share a cell of ⌈32/16⌉ = 2: trading them keeps its sum, not its
        // weighted sum.
        let row = |first: f32, second: f32| {
            let mut pixels = vec![0.0; 32];
            pixels[..2].copy_from_slice(&[first, second]);
            let mut snapshot = Snapshot::default();
            snapshot.plane("r", &Buffer2::new((32, 1), pixels));
            snapshot
        };
        let swapped = row(0.0, 1.0).differences(&row(1.0, 0.0));
        assert_eq!(swapped.len(), 1, "{swapped:?}");
        assert!(
            swapped[0].starts_with("r weighted cell (0, 0) is "),
            "{swapped:?}"
        );

        let plane = |pixel: f32| {
            let mut snapshot = Snapshot::default();
            snapshot.plane("p", &Buffer2::new((4, 1), vec![0.0, 0.0, pixel, 0.0]));
            snapshot
        };
        assert_eq!(
            plane(0.5).differences(&plane(0.0)),
            [
                "p max is 5e-1, the reference 0e0: 5.00e-1 off",
                "p cell (2, 0) is 5e-1, the reference 0e0: 5.00e-1 off",
                &format!(
                    "p weighted cell (2, 0) is {:e}, the reference 0e0: {:.2e} off",
                    0.5 * weight(2),
                    0.5 * weight(2)
                ),
            ]
        );

        let mut counted = Snapshot::default();
        counted.count("n", 3);
        let mut other = Snapshot::default();
        other.count("n", 4);
        assert_eq!(counted.differences(&other), ["n is 3, the reference 4"]);
        let mut digest = Snapshot::default();
        digest.digest("n", &[1, 2]);
        assert_eq!(
            digest.differences(&counted),
            ["n is a digest, the reference a count"]
        );
        let mut moved = Snapshot::default();
        moved.digest("n", &[1, 3]);
        assert_eq!(
            moved.differences(&digest),
            ["n: the digest is not the reference's"]
        );
        let mut renamed = Snapshot::default();
        renamed.count("m", 3);
        assert_eq!(renamed.differences(&counted), ["m, the reference holds n"]);
        let mut wide = Snapshot::default();
        wide.plane("p", &Buffer2::new((5, 1), vec![0.0; 5]));
        assert_eq!(
            wide.differences(&plane(0.0)),
            ["p is 5 × 1, the reference 4 × 1"]
        );
        assert_eq!(
            Snapshot::default().differences(&counted),
            ["0 entries, the reference holds 1"]
        );
    }
}
