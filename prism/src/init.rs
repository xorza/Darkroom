use anyhow::Result;
use std::sync::OnceLock;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_rolling_file::RollingFileAppenderBase;
use tracing_subscriber::fmt::writer::MakeWriterExt;

static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

pub fn init() -> Result<()> {
    dotenv::dotenv().ok();
    if let Err(e) = init_trace() {
        eprintln!("log init failed: {e}");
    }
    Ok(())
}

/// Best-effort tracing setup. Any failure is reported to stderr by
/// the caller and the app continues without file logging — logging
/// is a quality-of-life feature, not required for the editor to run.
fn init_trace() -> Result<()> {
    std::fs::create_dir_all("log")?;
    let appender = RollingFileAppenderBase::builder()
        .filename("log/editor.log".to_string())
        .max_filecount(10)
        .condition_max_file_size(10 * 1024 * 1024)
        .build()
        .map_err(|e| anyhow::anyhow!("build log appender: {e}"))?;
    let (non_blocking, log_guard) = appender.get_non_blocking_appender();
    LOG_GUARD
        .set(log_guard)
        .map_err(|_| anyhow::anyhow!("log guard already set"))?;
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_writer(non_blocking.and(std::io::stdout))
        .init();

    Ok(())
}
