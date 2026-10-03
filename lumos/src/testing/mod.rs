//! Testing utilities for lumos.

use std::any::Any;

pub(crate) mod assertions;
pub(crate) mod cfa;
mod characterization;
pub(crate) mod fits;
pub(crate) mod images;
pub(crate) mod mem_probe;
pub(crate) mod prelude;
#[cfg(feature = "real-data")]
pub(crate) mod real_data;
pub(crate) mod simd_check;
pub(crate) mod synthetic;
pub(crate) mod test_rng;
pub(crate) mod visual;

/// The text a caught panic carried: its `&str` or `String` payload, the two `panic!` produces.
pub(crate) fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .expect("panic! carries a &str or a String")
}

/// Initialize tracing subscriber for tests.
/// Safe to call multiple times - will only initialize once.
/// Respects `RUST_LOG` env var, defaults to "info".
pub(crate) fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    // `ort` (ONNX Runtime) logs its arena allocations at INFO — far too chatty; quiet it by default.
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,ort=warn"));
    #[expect(
        clippy::let_underscore_must_use,
        reason = "every test calls this, and only the first in a process installs the subscriber"
    )]
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_test_writer()
        .try_init();
}
