//! Testing utilities for lumos.

pub(crate) mod assertions;
pub(crate) mod cfa;
mod characterization;
pub(crate) mod images;
pub(crate) mod mem_probe;
pub(crate) mod prelude;
#[cfg(feature = "real-data")]
pub(crate) mod real_data;
pub(crate) mod simd_check;
pub(crate) mod synthetic;
pub(crate) mod test_rng;
pub(crate) mod visual;

/// Initialize tracing subscriber for tests.
/// Safe to call multiple times - will only initialize once.
/// Respects `RUST_LOG` env var, defaults to "info".
pub(crate) fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    // `ort` (ONNX Runtime) logs its arena allocations at INFO — far too chatty; quiet it by default.
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,ort=warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_test_writer()
        .try_init();
}
