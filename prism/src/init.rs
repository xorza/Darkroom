use anyhow::Result;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_rolling_file::RollingFileAppenderBase;
use tracing_subscriber::fmt::writer::MakeWriterExt;

/// Returns the non-blocking writer's `WorkerGuard`. Hold it in `main`
/// so it drops on normal exit and flushes any buffered log lines.
/// `None` means logging setup failed (already reported to stderr) —
/// the app continues without file logging.
pub fn init() -> Option<WorkerGuard> {
    dotenv::dotenv().ok();
    match init_trace() {
        Ok(guard) => Some(guard),
        Err(e) => {
            eprintln!("log init failed: {e}");
            None
        }
    }
}

fn init_trace() -> Result<WorkerGuard> {
    std::fs::create_dir_all("log")?;
    let appender = RollingFileAppenderBase::builder()
        .filename("log/editor.log".to_string())
        .max_filecount(10)
        .condition_max_file_size(10 * 1024 * 1024)
        .build()
        .map_err(|e| anyhow::anyhow!("build log appender: {e}"))?;
    let (non_blocking, log_guard) = appender.get_non_blocking_appender();
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_writer(non_blocking.and(std::io::stdout))
        .init();

    Ok(log_guard)
}
