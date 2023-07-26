use std::default::Default;
use std::path::Path;

use flexi_logger::{Duplicate, FileSpec, Logger};

#[macro_use]
pub mod macros;
pub mod scoped_ref;
pub mod apply;
pub mod toggle;


pub const EPSILON: f64 = 1e-10;

pub fn get_file_extension(filename: &str) -> anyhow::Result<&str> {
    let extension = Path::new(filename)
        .extension()
        .and_then(|os_str| os_str.to_str())
        .ok_or(anyhow::anyhow!("Failed to get file extension"))?;

    Ok(extension)
}

pub fn is_debug() -> bool {
    cfg!(debug_assertions)
}

pub fn setup_logging(base_level: &str) {
    let _ = Logger::try_with_str(base_level)
        .unwrap_or_else(|e| panic!("Logger initialization failed with {}", e))
        .log_to_file(
            FileSpec::default()
                .directory("logs")
        )
        .duplicate_to_stderr(Duplicate::Warn)
        .duplicate_to_stdout(Duplicate::All)
        .rotate(
            flexi_logger::Criterion::Size(1024 * 1024), //1MB
            flexi_logger::Naming::Timestamps,
            flexi_logger::Cleanup::KeepLogFiles(5),
        )
        .start()
        .unwrap_or_else(|e| panic!("Logger initialization failed with {}", e));
}