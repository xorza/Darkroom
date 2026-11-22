//! Testing utilities for lumos: the synthetic data and helpers the tests and the benches use.
//!
//! The one exception to gating test code by its callers: this tree is fixtures, compiled whole
//! wherever `internals` is on, and a bench build uses only part of them.
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "a bench build uses part of the test fixtures")
)]

#[cfg(test)]
pub(crate) mod assertions;
pub(crate) mod cfa;
#[cfg(test)]
mod characterization;
pub(crate) mod fits;
pub(crate) mod images;
pub(crate) mod invariance;
#[cfg(all(test, feature = "real-data"))]
pub(crate) mod real_data;
#[cfg(test)]
pub(crate) mod simd_check;
pub(crate) mod synthetic;
pub(crate) mod test_rng;
#[cfg(test)]
pub(crate) mod visual;

/// The text a caught panic carried: its `&str` or `String` payload, the two `panic!` produces.
#[cfg(test)]
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .expect("panic! carries a &str or a String")
}

/// Route the tests' logs to the test harness's output, at `RUST_LOG`'s filter or `info`; a second
/// call leaves the first logger in place.
#[cfg(test)]
pub(crate) fn init_tracing() {
    // `ort` (ONNX Runtime) logs its arena allocations at INFO — far too chatty; quiet it by
    // default.
    common::Logger::install_from_env("info,ort=warn", common::LogOutput::Test, None);
}
