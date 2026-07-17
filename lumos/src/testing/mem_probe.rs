//! Shared primitives for the `#[ignore]`d live memory probes (`mem_budget_probe` in
//! `stacking::combine` and `stacking::star_detection`).
//!
//! Those probes run a pipeline at scale and *watch* peak resident memory to prove it stays bounded.
//! The reusable parts live here — the MiB unit, env-var config parsing, and the background peak-RSS
//! sampler — so each probe keeps only its domain-specific setup (frame generation, config, and the
//! ceiling it asserts).
//!
//! Heap is read from `/proc/self/status`, so measurement is Linux-only; elsewhere every peak reads
//! 0 and probes skip their numeric assertion (the pipeline still runs).

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use fits_well::FitsWriter;
use fits_well::image::Image;
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rayon::prelude::*;

/// Bytes per MiB — probes report and budget in MiB.
pub(crate) const MB: u64 = 1024 * 1024;

/// How often [`RssSampler`] polls `/proc/self/status`: fine enough to catch a short allocation
/// burst, coarse enough to add no measurable load.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(2);

/// Parse env var `key` as `T`, falling back to `default` when it is unset or unparseable.
pub(crate) fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Read a `/proc/self/status` field (`RssAnon`, `VmRSS`, …) in KiB. Linux-only; returns 0 elsewhere,
/// so off Linux every sampled peak is 0 and probes skip their numeric assertion.
fn status_kb(field: &str) -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix(field).and_then(|r| r.strip_prefix(':')) {
            return rest
                .trim()
                .trim_end_matches("kB")
                .trim()
                .parse()
                .unwrap_or(0);
        }
    }
    0
}

/// A cloneable switch a probe flips once to mark a phase transition (load→combine, warmup→steady),
/// so [`RssSampler`] can attribute heap peaks to before vs after it. Cloneable and `Send`/`Sync` so
/// it can be captured by a progress callback that runs on another thread.
#[derive(Debug, Clone)]
pub(crate) struct PhaseGate(Arc<AtomicBool>);

impl PhaseGate {
    /// Open the gate: subsequent samples count toward [`PeakRss::gated_anon_mb`]. Idempotent.
    pub(crate) fn open(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Peak resident-memory high-water marks from one probe run, in MiB.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PeakRss {
    /// Peak `RssAnon` (heap — the OOM-relevant figure) over the whole run.
    pub anon_mb: u64,
    /// Peak `VmRSS` (total resident, including reclaimable mmap pages) over the whole run.
    pub total_mb: u64,
    /// Peak `RssAnon` sampled while the [`PhaseGate`] was open (e.g. combine / steady state).
    pub gated_anon_mb: u64,
    /// Peak `RssAnon` sampled while the [`PhaseGate`] was closed (e.g. load / warmup).
    pub ungated_anon_mb: u64,
}

/// Background sampler of peak heap (`RssAnon`) and total resident (`VmRSS`) for the duration of a
/// probe. [`start`](Self::start) spawns the polling thread with the gate closed;
/// [`finish`](Self::finish) stops it and returns the peaks. The [`PhaseGate`] from [`gate`](Self::gate)
/// splits the `RssAnon` peak into before/after the transition the probe marks.
#[derive(Debug)]
pub(crate) struct RssSampler {
    stop: Arc<AtomicBool>,
    gate: PhaseGate,
    peak_anon: Arc<AtomicU64>,
    peak_total: Arc<AtomicU64>,
    peak_gated: Arc<AtomicU64>,
    peak_ungated: Arc<AtomicU64>,
    handle: JoinHandle<()>,
}

impl RssSampler {
    /// Spawn the polling thread and start sampling immediately, gate closed.
    pub(crate) fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let gate = PhaseGate(Arc::new(AtomicBool::new(false)));
        let peak_anon = Arc::new(AtomicU64::new(0));
        let peak_total = Arc::new(AtomicU64::new(0));
        let peak_gated = Arc::new(AtomicU64::new(0));
        let peak_ungated = Arc::new(AtomicU64::new(0));
        let handle = {
            let stop = stop.clone();
            let gate = gate.0.clone();
            let (peak_anon, peak_total, peak_gated, peak_ungated) = (
                peak_anon.clone(),
                peak_total.clone(),
                peak_gated.clone(),
                peak_ungated.clone(),
            );
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let anon = status_kb("RssAnon");
                    peak_anon.fetch_max(anon, Ordering::Relaxed);
                    peak_total.fetch_max(status_kb("VmRSS"), Ordering::Relaxed);
                    if gate.load(Ordering::Relaxed) {
                        peak_gated.fetch_max(anon, Ordering::Relaxed);
                    } else {
                        peak_ungated.fetch_max(anon, Ordering::Relaxed);
                    }
                    std::thread::sleep(SAMPLE_INTERVAL);
                }
            })
        };
        Self {
            stop,
            gate,
            peak_anon,
            peak_total,
            peak_gated,
            peak_ungated,
            handle,
        }
    }

    /// A clone of the phase gate, to hand to whatever code marks the transition.
    pub(crate) fn gate(&self) -> PhaseGate {
        self.gate.clone()
    }

    /// Stop sampling, join the thread, and return the peaks converted KiB → MiB.
    pub(crate) fn finish(self) -> PeakRss {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.join().ok();
        PeakRss {
            anon_mb: self.peak_anon.load(Ordering::Relaxed) / 1024,
            total_mb: self.peak_total.load(Ordering::Relaxed) / 1024,
            gated_anon_mb: self.peak_gated.load(Ordering::Relaxed) / 1024,
            ungated_anon_mb: self.peak_ungated.load(Ordering::Relaxed) / 1024,
        }
    }
}

/// One synthetic 16-bit mono frame: a vignetted background pedestal + Gaussian read noise, plus a
/// handful of bright per-frame outliers (cosmic-ray stand-ins). The outliers differ every frame, so
/// sigma-clipping and median combine actually have something to reject — a mean combine would keep
/// them. Deterministic in `(seed, frame_idx)`; rows are generated in parallel with per-row RNGs.
pub(crate) fn synth_frame_u16(
    width: usize,
    height: usize,
    frame_idx: usize,
    seed: u64,
) -> Vec<u16> {
    const PEDESTAL: f32 = 0.08;
    const READ_NOISE: f32 = 0.012;

    let cx = width as f32 / 2.0;
    let cy = height as f32 / 2.0;
    let max_r2 = cx * cx + cy * cy;

    let mut data = vec![0u16; width * height];
    let frame_mix = seed ^ (frame_idx as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);

    data.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        let mut rng =
            ChaCha8Rng::seed_from_u64(frame_mix ^ (y as u64).wrapping_mul(0x0000_0100_0000_0001));
        let dy = y as f32 - cy;
        for (x, px) in row.iter_mut().enumerate() {
            let dx = x as f32 - cx;
            // Radial vignette in ~[0.75, 1.0] and a mild diagonal gradient give the field structure.
            let vignette = 1.0 - 0.25 * (dx * dx + dy * dy) / max_r2;
            let gradient = 0.03 * (x as f32 / width as f32 + y as f32 / height as f32);
            // Irwin–Hall (four uniforms) ≈ zero-mean Gaussian read noise.
            let g: f32 = rng.random::<f32>()
                + rng.random::<f32>()
                + rng.random::<f32>()
                + rng.random::<f32>()
                - 2.0;
            let v = PEDESTAL * vignette + gradient + READ_NOISE * g;
            *px = (v.clamp(0.0, 1.0) * 65535.0) as u16;
        }
    });

    let n_outliers = (width * height / 50_000).max(4);
    let mut rng = ChaCha8Rng::seed_from_u64(frame_mix.rotate_left(17));
    for _ in 0..n_outliers {
        let x = rng.random_range(0..width);
        let y = rng.random_range(0..height);
        let level = rng.random_range(0.85f32..1.0);
        data[y * width + x] = (level * 65535.0) as u16;
    }
    data
}

/// Write `data` as a 16-bit (BITPIX=16, BZERO=32768) FITS image, reusing `buf` as the encode scratch
/// so a whole stack allocates it once. `std::fs::write` flushes reliably (unlike a `BufWriter`
/// dropped inside the FITS writer).
pub(crate) fn write_fits_u16(
    path: &Path,
    width: usize,
    height: usize,
    data: &[u16],
    buf: &mut Vec<u8>,
) -> io::Result<()> {
    let image = Image::from_u16(vec![width, height], data).map_err(io::Error::other)?;
    buf.clear();
    FitsWriter::new(&mut *buf)
        .write_image(&image)
        .map_err(io::Error::other)?;
    std::fs::write(path, &buf)
}

/// A generated synthetic frame set: paths plus what it cost to materialize the missing ones.
#[derive(Debug)]
pub(crate) struct FrameSet {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) generated: usize,
    pub(crate) gen_secs: f64,
    pub(crate) bytes_on_disk: u64,
}

/// Generate `n` synthetic 16-bit FITS frames named `<prefix>_NNNN.fits` into `dir`, skipping any
/// already present (so a probe re-run reuses the cached set). Shared by the combine and pipeline
/// memory probes.
pub(crate) fn ensure_frames(
    dir: &Path,
    prefix: &str,
    n: usize,
    width: usize,
    height: usize,
    seed: u64,
) -> io::Result<FrameSet> {
    std::fs::create_dir_all(dir)?;
    let paths: Vec<PathBuf> = (0..n)
        .map(|i| dir.join(format!("{prefix}_{i:04}.fits")))
        .collect();

    let start = Instant::now();
    let mut buf = Vec::new();
    let mut generated = 0;
    for (i, path) in paths.iter().enumerate() {
        if path.exists() {
            continue;
        }
        let data = synth_frame_u16(width, height, i, seed);
        write_fits_u16(path, width, height, &data, &mut buf)?;
        generated += 1;
        print!("\r  generating {prefix} frames… {}/{}", i + 1, n);
        io::stdout().flush().ok();
    }
    if generated > 0 {
        println!();
    }
    let bytes_on_disk = paths
        .iter()
        .filter_map(|p| p.metadata().ok())
        .map(|m| m.len())
        .sum();

    Ok(FrameSet {
        paths,
        generated,
        gen_secs: start.elapsed().as_secs_f64(),
        bytes_on_disk,
    })
}

/// A resolved memory budget for a live probe: the `available_memory` to hand the stacker (or `None` =
/// query the system), plus a human label for the report. Parsed from a `LUMOS_*_BUDGET` env var by
/// [`parse_budget`]; the `ram`/`disk` sentinels force the resident/spill tier.
#[derive(Debug, Clone)]
pub(crate) struct BudgetChoice {
    pub(crate) available_memory: Option<u64>,
    pub(crate) label: String,
}

impl BudgetChoice {
    /// Query the system for available memory — no fixed budget, so no numeric ceiling is asserted.
    pub(crate) fn auto() -> Self {
        Self {
            available_memory: None,
            label: "auto (system available)".into(),
        }
    }

    /// A fixed budget of `mb` MiB.
    pub(crate) fn mb(mb: u64) -> Self {
        Self {
            available_memory: Some(mb * MB),
            label: format!("{mb} MB"),
        }
    }
}

/// Parse a `LUMOS_*_BUDGET` env var: `<N>` MiB, `ram` (force resident, `u64::MAX`), `disk` (force
/// spill, `1` byte), `auto` (query the system), or unset → `default`. Shared by the combine and
/// pipeline probes so budget semantics never drift between them.
pub(crate) fn parse_budget(env_key: &str, default: BudgetChoice) -> BudgetChoice {
    match std::env::var(env_key).ok().as_deref() {
        None | Some("") => default,
        Some("auto") => BudgetChoice::auto(),
        // u64::MAX ⇒ everything fits ⇒ in-memory tier. 1 byte ⇒ nothing fits ⇒ spill tier.
        Some("ram") => BudgetChoice {
            available_memory: Some(u64::MAX),
            label: "ram (force resident)".into(),
        },
        Some("disk") => BudgetChoice {
            available_memory: Some(1),
            label: "disk (force spill)".into(),
        },
        Some(n) => {
            let mb: u64 = n
                .parse()
                .unwrap_or_else(|_| panic!("{env_key}: expected ram|disk|auto|<MB>, got {n:?}"));
            BudgetChoice::mb(mb)
        }
    }
}

/// Whether a numeric peak-heap figure was actually measured. Off Linux `/proc/self/status` is absent,
/// so every sampled peak is 0; this prints a `<check> SKIPPED` line and returns false so the caller
/// skips its assertion. Shared by every live probe so the skip is worded identically.
pub(crate) fn measured(anon_mb: u64, check: &str) -> bool {
    if anon_mb == 0 {
        println!("{check} SKIPPED: no /proc/self/status (RssAnon unavailable off Linux)");
        false
    } else {
        true
    }
}

/// The MiB budget to assert peak heap against, or `None` when no numeric check applies: the
/// `auto`/`ram` sentinels aren't real ceilings, a budget below one in-flight decode's floor (~3× a
/// frame — one ~2× decode transient plus the output frame) can't be honored by any tiering, and off
/// Linux there's no measurement (prints a SKIPPED line via [`measured`]). Centralizes the shared skip
/// logic so each probe keeps only its own pass/fail message.
pub(crate) fn budget_ceiling_mb(
    anon_mb: u64,
    budget: &BudgetChoice,
    frame_bytes: u64,
) -> Option<u64> {
    let budget_bytes = budget.available_memory?;
    if budget_bytes == u64::MAX || budget_bytes < 3 * frame_bytes {
        return None;
    }
    measured(anon_mb, "budget check").then_some(budget_bytes / MB)
}

/// A RAM-path probe's peak-heap ceiling in MiB: `2 ×` the working set it should need — the
/// `resident_bytes` held for the whole run plus the `working_bytes` of concurrent transient scratch.
/// The 2× headroom absorbs allocator fragmentation and the sampler's coarse cadence; a per-frame leak
/// grows unboundedly and still blows it. Shared by the detection and align probes so both bound peak
/// the same way.
pub(crate) fn two_x_ceiling_mb(resident_bytes: u64, working_bytes: u64) -> u64 {
    2 * (resident_bytes + working_bytes) / MB
}
