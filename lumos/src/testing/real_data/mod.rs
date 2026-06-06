//! Real-data pipeline benchmark.
//!
//! Runs the full astrophotography pipeline: master darks/flats, calibration,
//! registration, and stacking using the best-precision configuration.
//!
//! Uses the bundled `test_data/lumos_data` directory
//! containing `Darks/`, `Flats/`, `Bias/`, and `Lights/` subdirectories with
//! raw frames.
//!
//! # Running
//!
//! ```bash
//! cargo test -p lumos --release bench_full_pipeline -- --ignored --nocapture
//! ```

mod pipeline_bench;
