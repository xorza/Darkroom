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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

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
