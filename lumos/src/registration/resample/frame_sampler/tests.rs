use crate::internals::prelude::*;
use crate::io::image::pixel_flags::PixelFlags;
use crate::registration::config::{self, InterpolationMethod};
use crate::registration::resample;
use crate::registration::resample::frame_sampler::{
    FilterSampling, FrameSampler, RowOutput, SampleMethod, SampleRow, WindowAxes,
};
use crate::registration::resample::kernel::internals::bicubic_kernel;
use crate::registration::resample::kernel::warp_kernel::{Filter, WarpKernel};
use crate::registration::resample::kernel::{self, LANCZOS_LUT_RESOLUTION, LanczosOrder};
use crate::registration::resample::masked_sources::MaskedSources;
use crate::registration::resample::ringing_clamp::RingingClamp;
use crate::registration::resample::source_image::{SourceImage, SourcePlane};
use crate::registration::resample::source_position::SourcePosition;
use crate::registration::transform::{Transform, WarpTransform};
use crate::simd::tier::Tier;

/// One output pixel as the rules define it, in f64 from the f32 tap weights.
#[derive(Debug, Clone)]
struct Expected {
    values: Vec<f64>,
    coverage: f64,
    confidence: f64,
    /// The rounding the engine's f32 sums may carry, per value.
    tolerance: f64,
}

/// A frame's planes, and its validity when it declares nulls: the sources the oracle reads.
#[derive(Debug)]
struct Oracle<'a> {
    planes: Vec<SourcePlane<'a>>,
    validity: Option<SourcePlane<'a>>,
    size: Size2us,
    border: f32,
}

/// Sums over a window's taps that hold data, and its whole magnitude.
#[derive(Debug, Default)]
struct OracleSums {
    total: f64,
    square: f64,
    magnitude: f64,
    whole: f64,
    positive: f64,
    negative: f64,
    taps: usize,
    all_in: bool,
}

impl Oracle<'_> {
    fn size(&self) -> Size2us {
        self.size
    }

    fn at(plane: SourcePlane<'_>, (x, y): (usize, usize)) -> f32 {
        plane.pixels[y * plane.width + x]
    }

    /// An in-bounds tap as a pixel index.
    fn index(x: i64, y: i64) -> (usize, usize) {
        (usize::try_from(x).unwrap(), usize::try_from(y).unwrap())
    }

    fn valid(&self, x: i64, y: i64) -> bool {
        let size = self.size();
        (0..size.width as i64).contains(&x)
            && (0..size.height as i64).contains(&y)
            && self
                .validity
                .is_none_or(|validity| Self::at(validity, Self::index(x, y)) == 1.0)
    }

    /// The taps of `filter` at `stretch` around `cell + frac` and their f32 weights, from the
    /// definitions: the fixed window of `2·radius` taps unstretched, every tap strictly inside the
    /// stretched radius otherwise.
    fn axis(filter: Filter, stretch: f32, cell: i32, frac: f32) -> Vec<(i64, f32)> {
        let radius = match filter {
            Filter::Bilinear => 1,
            Filter::Bicubic => 2,
            Filter::Lanczos(order) => order.a() as i32,
        };
        let reach = f64::from(radius as f32 * stretch);
        let taps: Vec<i32> = if stretch == 1.0 {
            (1 - radius..=radius).collect()
        } else {
            (-4 * radius..=4 * radius + 1)
                .filter(|&t| (f64::from(t) - f64::from(frac)).abs() < reach)
                .collect()
        };
        taps.into_iter()
            .map(|t| {
                let distance = (t as f32 - frac).abs();
                let weight = match filter {
                    Filter::Lanczos(order) => order
                        .lut()
                        .at(distance * (LANCZOS_LUT_RESOLUTION as f32 / stretch)),
                    Filter::Bicubic => bicubic_kernel(distance * (1.0 / stretch)),
                    Filter::Bilinear => (1.0 - distance * (1.0 / stretch)).max(0.0),
                };
                (i64::from(cell + t), weight)
            })
            .collect()
    }

    fn sums(&self, xs: &[(i64, f32)], ys: &[(i64, f32)]) -> OracleSums {
        let mut sums = OracleSums {
            all_in: true,
            taps: xs.len().max(ys.len()),
            ..OracleSums::default()
        };
        for &(y, wy) in ys {
            for &(x, wx) in xs {
                let weight = f64::from(wx) * f64::from(wy);
                sums.whole += weight.abs();
                if !self.valid(x, y) {
                    sums.all_in = false;
                    continue;
                }
                sums.total += weight;
                sums.square += weight * weight;
                sums.magnitude += weight.abs();
                if weight > 0.0 {
                    sums.positive += weight;
                } else {
                    sums.negative += weight;
                }
            }
        }
        sums
    }

    /// `Σ L·f` over the valid taps of `plane`, split as the clamp reads it: the light under each
    /// lobe and the part below zero.
    fn data(&self, plane: SourcePlane<'_>, xs: &[(i64, f32)], ys: &[(i64, f32)]) -> [f64; 4] {
        let [mut total, mut positive, mut negative, mut below] = [0.0; 4];
        for &(y, wy) in ys {
            for &(x, wx) in xs {
                if !self.valid(x, y) {
                    continue;
                }
                let weight = f64::from(wx) * f64::from(wy);
                let value = f64::from(Self::at(plane, Self::index(x, y)));
                total += weight * value;
                if weight > 0.0 {
                    positive += weight * value.max(0.0);
                } else {
                    negative += weight * value.max(0.0);
                }
                below += weight * value.min(0.0);
            }
        }
        [total, positive, negative, below]
    }

    fn largest(&self) -> f64 {
        self.planes
            .iter()
            .flat_map(|plane| plane.pixels)
            .fold(0.0f64, |m, &v| m.max(f64::from(v).abs()))
    }

    fn no_data(&self) -> Expected {
        Expected {
            values: vec![f64::from(self.border); self.planes.len()],
            coverage: 0.0,
            confidence: 0.0,
            tolerance: 0.0,
        }
    }

    fn sample(
        &self,
        position: Option<SourcePosition>,
        filter: Option<Stretched>,
        clamp: Option<f32>,
    ) -> Expected {
        let Some(position) = position else {
            return self.no_data();
        };
        let Some(Stretched { filter, stretch }) = filter else {
            let index = kernel::nearest_index(self.size(), position);
            let (x, y) = (index % self.size().width, index / self.size().width);
            if !self.valid(x as i64, y as i64) {
                return self.no_data();
            }
            return Expected {
                values: self
                    .planes
                    .iter()
                    .map(|&plane| f64::from(Self::at(plane, (x, y))))
                    .collect(),
                coverage: 1.0,
                confidence: 1.0,
                tolerance: 0.0,
            };
        };
        let xs = Self::axis(filter, stretch, position.cell_x, position.fx);
        let ys = Self::axis(filter, stretch, position.cell_y, position.fy);
        let sums = self.sums(&xs, &ys);
        let coverage = if sums.all_in {
            1.0
        } else {
            (sums.magnitude / sums.whole).min(1.0)
        };
        let accepted = sums.all_in || (sums.total > 0.0 && sums.total * sums.total >= sums.square);
        let (xs, ys, sums, clamp) = if accepted {
            (xs, ys, sums, clamp.filter(|_| filter != Filter::Bilinear))
        } else {
            let xs = Self::axis(Filter::Bilinear, stretch, position.cell_x, position.fx);
            let ys = Self::axis(Filter::Bilinear, stretch, position.cell_y, position.fy);
            let fallback = self.sums(&xs, &ys);
            if fallback.total <= 0.0 || coverage <= 0.0 {
                return self.no_data();
            }
            (xs, ys, fallback, None)
        };
        let n = (sums.taps * sums.taps) as f64;
        let tolerance =
            4.0 * n * f64::from(f32::EPSILON) * self.largest() * sums.magnitude / sums.total;
        let values = self
            .planes
            .iter()
            .map(|&plane| {
                let [total, positive, negative, below] = self.data(plane, &xs, &ys);
                let Some(threshold) = clamp else {
                    return total / sums.total;
                };
                let threshold = f64::from(threshold);
                let light = if positive == 0.0 {
                    0.0
                } else {
                    let ratio = -negative / positive;
                    if ratio >= 1.0 {
                        positive / sums.positive
                    } else {
                        let keep = if ratio > threshold {
                            1.0 - ((ratio - threshold) / (1.0 - threshold)).powi(2)
                        } else {
                            1.0
                        };
                        (positive + keep * negative) / (sums.positive + keep * sums.negative)
                    }
                };
                below / sums.total + light
            })
            .collect();
        Expected {
            values,
            coverage,
            confidence: sums.total * sums.total / sums.square,
            tolerance,
        }
    }
}

/// A `size` plane of signed, unstructured values with a few bright pixels among them.
fn fixture(size: Size2us, seed: usize) -> Buffer2<f32> {
    Buffer2::new(
        size.width,
        size.height,
        (0..size.pixel_count())
            .map(|i| {
                if (i + seed).is_multiple_of(37) {
                    40.0
                } else {
                    ((i * 13 + seed + i / size.width * 7) % 31) as f32 / 9.0 - 1.0
                }
            })
            .collect(),
    )
}

/// The positions of a rotated, enlarged grid over a `size` source: interior, edge band, the rim
/// and outside.
fn positions(size: Size2us) -> Vec<Option<SourcePosition>> {
    let mut positions = Vec::new();
    for j in 0..26 {
        for i in 0..34 {
            let p = DVec2::new(
                -1.7 + 0.83 * f64::from(i) + 0.11 * f64::from(j),
                -1.3 + 0.79 * f64::from(j) - 0.07 * f64::from(i),
            );
            positions.push(SourcePosition::within(p, size));
        }
    }
    positions
}

/// What [`sample`] returns: every channel's samples and the two quality values, per position.
#[derive(Debug)]
struct Sampled {
    channels: Vec<Vec<f32>>,
    coverage: Vec<f32>,
    confidence: Vec<f32>,
}

/// Every channel and the quality of `positions` sampled by `sampler` on `tier`.
fn sample(tier: Tier, sampler: &FrameSampler<'_>, positions: &[Option<SourcePosition>]) -> Sampled {
    let mut channels = vec![vec![f32::NAN; positions.len()]; sampler.sources.len()];
    let mut coverage = vec![f32::NAN; positions.len()];
    let mut confidence = vec![f32::NAN; positions.len()];
    let mut rows: Vec<&mut [f32]> = channels.iter_mut().map(Vec::as_mut_slice).collect();
    let mut axes = WindowAxes::default();
    tier.run(SampleRow {
        sampler,
        positions,
        axes: &mut axes,
        output: RowOutput {
            channels: &mut rows,
            coverage: &mut coverage,
            confidence: &mut confidence,
        },
    });
    Sampled {
        channels,
        coverage,
        confidence,
    }
}

/// A filter at a stretch, as the oracle reads it.
#[derive(Debug, Clone, Copy)]
struct Stretched {
    filter: Filter,
    stretch: f32,
}

/// One way of sampling, for the engine and for the oracle.
#[derive(Debug, Clone, Copy)]
struct Case {
    method: SampleMethod,
    /// `None` for Nearest.
    filter: Option<Stretched>,
    clamp: Option<f32>,
}

/// The methods the sampler offers: Nearest, and every filter at two stretches with the clamp on
/// and off.
fn methods() -> Vec<Case> {
    let mut methods = vec![Case {
        method: SampleMethod::Nearest,
        filter: None,
        clamp: None,
    }];
    let filters = [
        Filter::Bilinear,
        Filter::Bicubic,
        Filter::Lanczos(LanczosOrder::Two),
        Filter::Lanczos(LanczosOrder::Three),
        Filter::Lanczos(LanczosOrder::Four),
    ];
    for filter in filters {
        for stretch in [1.0, 1.3] {
            for clamp in [None, Some(0.3)] {
                let kernel = WarpKernel::new(filter, stretch);
                methods.push(Case {
                    method: SampleMethod::Filter(FilterSampling {
                        kernel,
                        clamp: clamp
                            .filter(|_| filter.has_negative_lobes())
                            .map(RingingClamp::new),
                    }),
                    filter: Some(Stretched { filter, stretch }),
                    clamp,
                });
            }
        }
    }
    methods
}

/// Every pixel matches the rules computed in f64 from the same tap weights — the whole kernel in
/// the interior, its in-bounds and valid taps normalized where they are well conditioned, Bilinear
/// over its own valid taps otherwise, the clamp where it is on, and the quality of the
/// coefficients used — for every method at two stretches, with and without the clamp and nulls,
/// across the interior, the edge band, the rim and outside.
///
/// The engine sums in f32: each of its sums rounds by up to `n²·ε` of its absolute sum, so a
/// normalized value by `4·n²·ε·max|f|·Σ|L|/Σ L`, the bound each pixel is held to. Coverage and
/// confidence are ratios of such sums over magnitudes of order 1, so `1e-4`.
#[test]
fn every_pixel_follows_the_rules() {
    let size = Size2us::new(24, 20);
    let planes = [fixture(size, 0), fixture(size, 5)];
    let mut nulls = vec![0.0f32; size.pixel_count()];
    for index in [5 * 24 + 7, 5 * 24 + 8, 12 * 24 + 15, 19 * 24, 9 * 24 + 23] {
        nulls[index] = f32::NAN;
    }
    let flags = PixelFlags::of_non_finite(size, &[&nulls]).unwrap();
    let positions = positions(size);
    let mut decided = [0usize; 3];
    for Case {
        method,
        filter,
        clamp,
    } in methods()
    {
        for masked in [false, true] {
            let rgb = LinearImage::from_planar_channels(
                ImageDimensions::new((size.width, size.height), 3),
                [
                    planes[0].pixels().to_vec(),
                    planes[1].pixels().to_vec(),
                    planes[0].pixels().to_vec(),
                ],
            );
            let source = SourceImage::of(&rgb);
            let sources = masked.then(|| MaskedSources::new(&source, &flags, method.reach()));
            let sampler = FrameSampler::new(method, &source, sources.as_ref(), -7.0);
            let oracle = Oracle {
                planes: sampler.sources.iter().copied().collect(),
                validity: sources.as_ref().map(MaskedSources::validity),
                size,
                border: -7.0,
            };
            let Sampled {
                channels,
                coverage,
                confidence,
            } = sample(Tier::portable(), &sampler, &positions);
            for (index, &position) in positions.iter().enumerate() {
                let expected = oracle.sample(position, filter, clamp);
                let what = format!("{filter:?} clamp {clamp:?} masked {masked} at {position:?}");
                for (channel, &value) in expected.values.iter().enumerate() {
                    let actual = f64::from(channels[channel][index]);
                    assert!(
                        (actual - value).abs() <= expected.tolerance,
                        "{what} channel {channel}: {actual} against {value}"
                    );
                }
                assert!(
                    (f64::from(coverage[index]) - expected.coverage).abs() <= 1e-4,
                    "{what}: coverage {} against {}",
                    coverage[index],
                    expected.coverage
                );
                assert!(
                    (f64::from(confidence[index]) - expected.confidence).abs()
                        <= 1e-4 * expected.confidence.max(1.0),
                    "{what}: confidence {} against {}",
                    confidence[index],
                    expected.confidence
                );
                decided[usize::from(expected.coverage > 0.0)
                    + usize::from(expected.coverage == 1.0)] += 1;
            }
        }
    }
    // The positions reach every case: no data, partial coverage and the whole kernel.
    assert!(decided.iter().all(|&count| count > 0), "{decided:?}");
}

/// Every tier samples every case to `Portable`'s bits: the fold of each sum is fixed, so a pixel
/// does not depend on the CPU.
#[test]
fn every_tier_samples_to_portables_bits() {
    let size = Size2us::new(24, 20);
    let rgb = LinearImage::from_planar_channels(
        ImageDimensions::new((size.width, size.height), 3),
        [
            fixture(size, 0).pixels().to_vec(),
            fixture(size, 3).pixels().to_vec(),
            fixture(size, 9).pixels().to_vec(),
        ],
    );
    let mut nulls = vec![0.0f32; size.pixel_count()];
    nulls[6 * 24 + 6] = f32::NAN;
    nulls[6 * 24 + 7] = f32::NAN;
    let flags = PixelFlags::of_non_finite(size, &[&nulls]).unwrap();
    let positions = positions(size);
    for Case { method, .. } in methods() {
        for masked in [false, true] {
            let source = SourceImage::of(&rgb);
            let sources = masked.then(|| MaskedSources::new(&source, &flags, method.reach()));
            let sampler = FrameSampler::new(method, &source, sources.as_ref(), -7.0);
            let reference = sample(Tier::portable(), &sampler, &positions);
            for tier in Tier::supported() {
                let sampled = sample(tier, &sampler, &positions);
                let bits = |values: &[f32]| values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
                for (channel, expected) in sampled.channels.iter().zip(&reference.channels) {
                    assert_eq!(
                        bits(channel),
                        bits(expected),
                        "{tier} {method:?} masked {masked}"
                    );
                }
                assert_eq!(
                    bits(&sampled.coverage),
                    bits(&reference.coverage),
                    "{tier} {method:?}"
                );
                assert_eq!(
                    bits(&sampled.confidence),
                    bits(&reference.confidence),
                    "{tier} {method:?}"
                );
            }
        }
    }
}

/// A Lanczos3 window on a source of `width × 9`: one row of `values` at row 4, the rest
/// `background`, sampled at `(x, 4 + fy)` with the clamp at `threshold`.
fn clamped_sample(values: &[f32], background: f32, position: DVec2, threshold: Option<f32>) -> f32 {
    let size = Size2us::new(values.len(), 9);
    let mut pixels = vec![background; size.pixel_count()];
    pixels[4 * size.width..5 * size.width].copy_from_slice(values);
    let image = LinearImage::from_pixels(ImageDimensions::new((size.width, 9), 1), pixels);
    let method = SampleMethod::Filter(FilterSampling {
        kernel: WarpKernel::new(Filter::Lanczos(LanczosOrder::Three), 1.0),
        clamp: threshold.map(RingingClamp::new),
    });
    let source = SourceImage::of(&image);
    let sampler = FrameSampler::new(method, &source, None, 0.0);
    let positions = [SourcePosition::within(position, size)];
    sample(Tier::portable(), &sampler, &positions).channels[0][0]
}

/// A bright pixel under Lanczos3's first negative lobe, by hand.
///
/// At a half-pixel phase in x on a row (phase 0 in y, so only the row itself carries weight, its
/// neighbours' weights being the table's residue at integer distances, under 1e-7), the six taps
/// weigh `L(2.5) = 0.0243`, `L(1.5) = −0.1351`, `L(0.5) = 0.6079` either side, `wₚ = 1.2645`,
/// `wₙ = −0.2702`. With a bright `V` at the tap 1.5 px away and `B` elsewhere:
/// - unclamped, `B + (V − B)·L(1.5)/(wₚ + wₙ)`: at `V = 1000`, `B = 10` that is `10 − 134.5`,
///   an undershoot of 13% of the peak below zero;
/// - clamped, `r = (0.2702·B + 0.1351·(V − B))/(1.2645·B)` = 10.8 ≥ 1, so the sample is the
///   positive lobes' mean, `B`: no undershoot at all;
/// - at `V = 36.8` the ratio is `(2.7019 + 26.8·0.13509)/12.6449 = 6.3224/12.6449 = 0.5`, past
///   0.3, so the negative lobes keep `c = 1 − (0.2/0.7)² = 45/49` of their weight:
///   `(12.6449 − c·6.3224)/(1.26449 − c·0.27019)` = `6.8386/1.01635` = 6.7285, against the
///   unclamped `(12.6449 − 6.3224)/0.99430` = 6.3588.
///
/// The expectations are computed from the table's own entries, the residue rows included, so the
/// f32 sums are the only difference: under `40·ε` of the window's absolute sum.
#[test]
fn a_bright_pixel_keeps_its_undershoot_within_the_clamp() {
    let lut = LanczosOrder::Three.lut();
    let entry = |distance: f32| f64::from(lut.at(distance * LANCZOS_LUT_RESOLUTION as f32));
    let row = [
        entry(2.5),
        entry(1.5),
        entry(0.5),
        entry(0.5),
        entry(1.5),
        entry(2.5),
    ];
    let (positive, negative) = (
        row.iter().filter(|&&w| w > 0.0).sum::<f64>(),
        row.iter().filter(|&&w| w < 0.0).sum::<f64>(),
    );
    let total = positive + negative;
    // Output sample at x = 8.5 reads taps 6..=11; the bright pixel at 7 is 1.5 px away.
    let line = |bright: f32, background: f32| {
        let mut values = vec![background; 16];
        values[7] = bright;
        values
    };
    let position = DVec2::new(8.5, 4.0);
    let tolerance = |scale: f64| 40.0 * f64::from(f32::EPSILON) * scale;

    let (bright, background) = (1000.0, 10.0);
    let unclamped = clamped_sample(&line(bright, background), background, position, None);
    let expected = f64::from(background) + f64::from(bright - background) * row[1] / total;
    assert!(
        (f64::from(unclamped) - expected).abs() < tolerance(1000.0) + 1e-4,
        "{unclamped} against {expected}"
    );
    assert!(unclamped < -120.0, "{unclamped}");
    let clamped = clamped_sample(&line(bright, background), background, position, Some(0.3));
    assert!((clamped - background).abs() < 1e-3, "{clamped}");

    let soft_bright = 36.8;
    let light_positive = f64::from(background) * positive;
    let light_negative =
        f64::from(background) * negative + f64::from(soft_bright - background) * row[1];
    let ratio = -light_negative / light_positive;
    assert!((ratio - 0.5).abs() < 1e-3, "{ratio}");
    let keep = 1.0 - ((ratio - 0.3) / 0.7).powi(2);
    let expected = (light_positive + keep * light_negative) / (positive + keep * negative);
    let soft = clamped_sample(
        &line(soft_bright, background),
        background,
        position,
        Some(0.3),
    );
    assert!(
        (f64::from(soft) - expected).abs() < tolerance(40.0) + 1e-5,
        "{soft} against {expected}"
    );
    assert!((soft - 6.7285).abs() < 1e-3, "{soft}");
}

/// The masked fixture of the review, two adjacent nulls under a half-pixel sample, by hand.
///
/// Lanczos3 at (8.5, 8.5) loses its two heaviest taps, `L(½)² = 0.3696` each: what is left sums to
/// `0.9886 − 0.7391 = 0.249` while its squares sum to 0.330, so normalizing by it would amplify
/// the noise by `0.330/0.249² = 5.3` pixels' worth. Bilinear takes over: its two valid taps of
/// four, 0.25 each, average `f(8, 9) = 98` and `f(9, 9) = 99` on `f = x + 10y` to 98.5, with
/// confidence `0.5²/(2·0.25²) = 2`. Coverage stays Lanczos3's: the valid share of its magnitude,
/// `1 − 2·L(½)²/(Σ|L|)²` with `Σ|L| = 2·(0.6079 + 0.1351 + 0.0243) = 1.5346`, which is 0.686.
#[test]
fn two_adjacent_nulls_fall_back_to_bilinear() {
    let size = Size2us::new(18, 18);
    let ramp: Vec<f32> = (0..size.pixel_count())
        .map(|i| (i % 18) as f32 + 10.0 * (i / 18) as f32)
        .collect();
    let mut nulls = vec![0.0f32; size.pixel_count()];
    nulls[8 * 18 + 8] = f32::NAN;
    nulls[8 * 18 + 9] = f32::NAN;
    let mut image = LinearImage::from_pixels(ImageDimensions::new((18, 18), 1), ramp);
    image.flags = PixelFlags::of_non_finite(size, &[&nulls]);
    let method = SampleMethod::Filter(FilterSampling {
        kernel: WarpKernel::new(Filter::Lanczos(LanczosOrder::Three), 1.0),
        clamp: Some(RingingClamp::new(0.3)),
    });
    let source = SourceImage::of(&image);
    let sources = MaskedSources::new(&source, image.flags.as_ref().unwrap(), method.reach());
    let sampler = FrameSampler::new(method, &source, Some(&sources), -7.0);
    let positions = [SourcePosition::within(DVec2::new(8.5, 8.5), size)];
    let Sampled {
        channels,
        coverage,
        confidence,
    } = sample(Tier::portable(), &sampler, &positions);
    assert_eq!(channels[0][0], 98.5);
    assert_eq!(confidence[0], 2.0);

    let lut = LanczosOrder::Three.lut();
    let entry = |distance: f32| f64::from(lut.at(distance * LANCZOS_LUT_RESOLUTION as f32));
    let magnitude = 2.0 * (entry(0.5) + entry(1.5).abs() + entry(2.5));
    let expected = 1.0 - 2.0 * entry(0.5).powi(2) / (magnitude * magnitude);
    assert!(
        (f64::from(coverage[0]) - expected).abs() < 1e-6,
        "{} against {expected}",
        coverage[0]
    );
    assert!((coverage[0] - 0.686).abs() < 1e-3, "{}", coverage[0]);
}

/// The two quality values reach zero together, and coverage never leaves `[0, 1]`, for every
/// method across every border, the rim and the outside — the pairing the combine leans on.
#[test]
fn coverage_and_confidence_vanish_together() {
    let size = Size2us::new(24, 20);
    let rgb = LinearImage::from_pixels(
        ImageDimensions::new((size.width, size.height), 1),
        fixture(size, 1).pixels().to_vec(),
    );
    let positions = positions(size);
    for method in InterpolationMethod::ALL {
        let sample_method = SampleMethod::for_frame(
            config::internals::warp_params(method),
            &WarpTransform::new(Transform::identity()),
            size,
        );
        let source = SourceImage::of(&rgb);
        let sampler = FrameSampler::new(sample_method, &source, None, 0.0);
        let sampled = sample(Tier::portable(), &sampler, &positions);
        for (&coverage, &confidence) in sampled.coverage.iter().zip(&sampled.confidence) {
            assert_eq!(
                coverage == 0.0,
                confidence == 0.0,
                "{method:?}: {coverage}, {confidence}"
            );
            assert!((0.0..=1.0).contains(&coverage), "{method:?}: {coverage}");
        }
    }
}

/// A Nyquist grating shrunk to half size aliases to a flat field unless the kernel is stretched.
///
/// Columns alternate 2 and 0, `1 + cos(πx)`. A warp with output-to-source scale 2 samples every
/// even column at phase 0, so the unstretched Lanczos3, its centre tap 1 and the rest the table's
/// residue, reads 2 everywhere: the grating folds to an offset of its whole amplitude. Stretched
/// by 2, the taps `t = −5..=5` weigh `L(|t|/2)` — 1, 0.6079, 0, −0.1351, 0, 0.0243 from the centre
/// out — and the sample is `1 + Σ(−1)ᵗL(t/2)/ΣL(t/2)` = `1 + (1 − 1.2158 + 0.2702 − 0.0486)/
/// (1 + 1.2158 − 0.2702 + 0.0486)` = `1 + 0.0058/1.9942` = 1.0029: the alias is 0.3% of the
/// grating. The warp finds the stretch from the transform itself. The expectations come from the
/// table's own entries; the f32 sums of up to 121 terms of at most 2 round within `121·ε·2·2`.
#[test]
fn a_halved_nyquist_grating_does_not_alias() {
    let size = Size2us::new(64, 64);
    let grating: Vec<f32> = (0..size.pixel_count())
        .map(|i| if (i % 64).is_multiple_of(2) { 2.0 } else { 0.0 })
        .collect();
    let image = LinearImage::from_pixels(ImageDimensions::new((64, 64), 1), grating);
    let halving = WarpTransform::new(Transform::similarity(DVec2::ZERO, 0.0, 2.0));
    let warped = resample::warp(
        &image,
        &halving,
        config::internals::warp_params(InterpolationMethod::Lanczos3),
    );

    let lut = LanczosOrder::Three.lut();
    let entry = |distance: f32| f64::from(lut.at(distance * LANCZOS_LUT_RESOLUTION as f32));
    let (alternating, total) = (-5i32..=5).fold((0.0, 0.0), |(alternating, total), t| {
        let weight = entry(t.unsigned_abs() as f32 / 2.0);
        let sign = if t % 2 == 0 { 1.0 } else { -1.0 };
        (alternating + sign * weight, total + weight)
    });
    let expected = 1.0 + alternating / total;
    assert!((expected - 1.0029).abs() < 1e-4, "{expected}");
    let tolerance = 121.0 * f64::from(f32::EPSILON) * 2.0 * 2.0;
    // The output pixels whose stretched window, ±5 source pixels about `2x`, lies in the source.
    for y in 3..=29 {
        for x in 3..=29 {
            let value = f64::from(warped.image.channel(0)[(x, y)]);
            assert!(
                (value - expected).abs() <= tolerance,
                "({x}, {y}): {value} against {expected}"
            );
        }
    }

    let unstretched = SampleMethod::Filter(FilterSampling {
        kernel: WarpKernel::new(Filter::Lanczos(LanczosOrder::Three), 1.0),
        clamp: Some(RingingClamp::new(0.3)),
    });
    let source = SourceImage::of(&image);
    let sampler = FrameSampler::new(unstretched, &source, None, 0.0);
    let positions = [SourcePosition::within(DVec2::new(20.0, 20.0), size)];
    let aliased = sample(Tier::portable(), &sampler, &positions).channels[0][0];
    assert!((aliased - 2.0).abs() < 1e-5, "{aliased}");
}

/// Bilinear reproduces a plane and Catmull-Rom a quadratic — any `xⁱyʲ` with `i, j ≤ 2`, since its
/// 1-D weights reproduce `1, x, x²` — at interior fractions, to rounding; and every method, the
/// clamp on, reads a constant below zero back anywhere it has data, the clamp passing what lies
/// below zero through the plain kernel.
///
/// The rounding: each f32 sum of up to 16 terms rounds by `16·ε` of its absolute sum, at most
/// `Σ|w|·max|v|`, with Catmull-Rom's `Σ|w|` largest at `f = ½`, `(2·0.5625 + 2·0.0625)² = 1.5625`;
/// a ratio of two sums doubles it. A constant through a normalized window is off by at most
/// `2·64·ε·8` of itself (see `an_image_smaller_than_the_kernel_reads_a_constant_back`). Bilinear
/// does not reproduce the quadratic, which is what makes the second claim a test: at a cell's
/// centre it averages four corners, high by `(0.02 + 0.05)/4 = 0.0175` on this one.
#[test]
fn filters_reproduce_their_polynomials() {
    let size = Size2us::new(12, 10);
    let plane = |x: f64, y: f64| 0.25 * x - 0.5 * y + 3.0;
    let quadratic =
        |x: f64, y: f64| 0.02 * x * x - 0.03 * x * y + 0.05 * y * y + 0.1 * x - 0.2 * y + 1.0;
    let image = |field: &dyn Fn(f64, f64) -> f64| {
        LinearImage::from_pixels(
            ImageDimensions::new((size.width, size.height), 1),
            (0..size.pixel_count())
                .map(|i| field((i % size.width) as f64, (i / size.width) as f64) as f32)
                .collect(),
        )
    };
    let unclamped = |filter| {
        SampleMethod::Filter(FilterSampling {
            kernel: WarpKernel::new(filter, 1.0),
            clamp: None,
        })
    };
    let at = |method, image: &LinearImage, p: DVec2| {
        let source = SourceImage::of(image);
        let sampler = FrameSampler::new(method, &source, None, 0.0);
        sample(
            Tier::portable(),
            &sampler,
            &[SourcePosition::within(p, size)],
        )
        .channels[0][0]
    };
    let (plane_image, quadratic_image) = (image(&plane), image(&quadratic));
    let largest = |image: &LinearImage| {
        image
            .channel(0)
            .pixels()
            .iter()
            .fold(0.0f64, |m, &v| m.max(f64::from(v).abs()))
    };
    let epsilon = f64::from(f32::EPSILON);
    let plane_bound = 2.0 * 16.0 * epsilon * largest(&plane_image);
    let quadratic_bound = 2.0 * 16.0 * epsilon * 1.5625 * largest(&quadratic_image);
    for (fx, fy) in [(0.125, 0.375), (0.5, 0.5), (0.8, 0.1)] {
        for cell_y in 1..size.height - 2 {
            for cell_x in 1..size.width - 2 {
                let p = DVec2::new(cell_x as f64 + fx, cell_y as f64 + fy);
                let split = SourcePosition::within(p, size).unwrap();
                // The kernels sample the narrowed fraction, so the truth is taken there.
                let (x, y) = (
                    f64::from(split.cell_x) + f64::from(split.fx),
                    f64::from(split.cell_y) + f64::from(split.fy),
                );
                let linear = f64::from(at(unclamped(Filter::Bilinear), &plane_image, p));
                assert!(
                    (linear - plane(x, y)).abs() <= plane_bound,
                    "Bilinear at {p:?}: {linear} against {}",
                    plane(x, y)
                );
                let cubic = f64::from(at(unclamped(Filter::Bicubic), &quadratic_image, p));
                assert!(
                    (cubic - quadratic(x, y)).abs() <= quadratic_bound,
                    "Bicubic at {p:?}: {cubic} against {}",
                    quadratic(x, y)
                );
            }
        }
    }
    let centre = DVec2::new(5.5, 4.5);
    let overshoot =
        f64::from(at(unclamped(Filter::Bilinear), &quadratic_image, centre)) - quadratic(5.5, 4.5);
    assert!(
        (overshoot - 0.0175).abs() <= quadratic_bound,
        "Bilinear on the quadratic is off by {overshoot}"
    );

    let constant = image(&|_, _| -2.5);
    let bound = 2.0 * 64.0 * epsilon * 8.0 * 2.5;
    for method in InterpolationMethod::ALL {
        let sample_method = SampleMethod::for_frame(
            config::internals::warp_params(method),
            &WarpTransform::new(Transform::identity()),
            size,
        );
        let source = SourceImage::of(&constant);
        let sampler = FrameSampler::new(sample_method, &source, None, 9.0);
        let sampled = sample(Tier::portable(), &sampler, &positions(size));
        for (&value, &coverage) in sampled.channels[0].iter().zip(&sampled.coverage) {
            if coverage > 0.0 {
                assert!(
                    (f64::from(value) + 2.5).abs() <= bound,
                    "{method:?}: {value} at coverage {coverage}"
                );
            } else {
                assert_eq!(value, 9.0, "{method:?}");
            }
        }
    }
}
