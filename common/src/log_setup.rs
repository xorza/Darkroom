use flexi_logger::{
    style, DeferredNow, Duplicate, FileSpec, Logger, TS_DASHES_BLANK_COLONS_DOT_BLANK,
};
use log::Record;

pub fn detailed_format(
    w: &mut dyn std::io::Write,
    now: &mut DeferredNow,
    record: &Record,
) -> Result<(), std::io::Error> {
    write!(
        w,
        "[{}] {} [{}:{}]: {}",
        now.format(TS_DASHES_BLANK_COLONS_DOT_BLANK),
        record.level(),
        record.module_path().unwrap_or("<unnamed>"),
        record.line().unwrap_or(0),
        &record.args()
    )
}

pub fn colored_detailed_format(
    w: &mut dyn std::io::Write,
    now: &mut DeferredNow,
    record: &Record,
) -> Result<(), std::io::Error> {
    let level = record.level();
    write!(
        w,
        "[{}] {} [{}:{}]: {}",
        style(level).paint(now.format(TS_DASHES_BLANK_COLONS_DOT_BLANK).to_string()),
        style(level).paint(record.level().to_string()),
        record.module_path().unwrap_or("<unnamed>"),
        record.line().unwrap_or(0),
        style(level).paint(&record.args().to_string())
    )
}

pub fn setup_logging(base_level: &str) {
    let _ = Logger::try_with_str(base_level)
        .unwrap_or_else(|e| panic!("Logger initialization failed with {}", e))
        .log_to_file(FileSpec::default().directory("logs"))
        .duplicate_to_stderr(Duplicate::Warn)
        .duplicate_to_stdout(Duplicate::All)
        .format(detailed_format)
        .format_for_stdout(colored_detailed_format)
        .rotate(
            flexi_logger::Criterion::Size(1024 * 1024), //1MB
            flexi_logger::Naming::Timestamps,
            flexi_logger::Cleanup::KeepLogFiles(5),
        )
        .start()
        .unwrap_or_else(|e| panic!("Logger initialization failed with {}", e));
}
