use anyhow::Result;
use std::sync::OnceLock;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_rolling_file::RollingFileAppenderBase;
use tracing_subscriber::fmt::writer::MakeWriterExt;

static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

pub fn init() -> Result<()> {
    dotenv::dotenv().ok();
    init_trace().ok();

    Ok(())
}

fn init_trace() -> Result<()> {
    std::fs::create_dir_all("log")?;
    let appender = RollingFileAppenderBase::builder()
        .filename("log/editor.log".to_string())
        .max_filecount(10)
        .condition_max_file_size(10 * 1024 * 1024)
        .build()
        .expect("failed to initialize log appender");
    let (non_blocking, log_guard) = appender.get_non_blocking_appender();
    LOG_GUARD
        .set(log_guard)
        .expect("log guard should only be initialized once");
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_writer(non_blocking.and(std::io::stdout))
        .init();

    Ok(())
}
