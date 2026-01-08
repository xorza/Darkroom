use anyhow::Result;
use tracing_rolling_file::RollingFileAppenderBase;

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
    let (non_blocking, _log_guard) = appender.get_non_blocking_appender();
    tracing_subscriber::fmt()
        .with_env_filter("debug")
        .with_writer(non_blocking)
        .with_writer(std::io::stdout)
        .init();

    Ok(())
}
