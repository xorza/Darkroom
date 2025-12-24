use std::sync::OnceLock;

use tracing::Level;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

pub fn setup_logging(base_level: &str) {
    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(base_level))
        .unwrap_or_else(|e| panic!("Invalid log filter: {}", e));

    std::fs::create_dir_all("logs")
        .unwrap_or_else(|e| panic!("Failed to create logs directory: {}", e));

    let file_appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("scenarium")
        .filename_suffix("log")
        .max_log_files(5)
        .build("logs")
        .unwrap_or_else(|e| panic!("Failed to create log file appender: {}", e));

    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);
    LOG_GUARD.set(guard).expect("Logging already initialized");

    let console_writer = std::io::stdout.and(std::io::stderr.with_min_level(Level::WARN));

    let console_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_line_number(true)
        .with_file(true)
        .with_ansi(true)
        .with_writer(console_writer);

    let file_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_line_number(true)
        .with_file(true)
        .with_ansi(false)
        .with_writer(file_writer);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(console_layer)
        .with(file_layer)
        .try_init()
        .unwrap_or_else(|e| panic!("Logger initialization failed: {}", e));
}
