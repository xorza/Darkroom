//! [`Logger`]: the subscriber for `tracing` and the logger for `log`, in one.

use std::io::{self, IsTerminal as _, Write as _};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::level_filters::LevelFilter;
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber, subscriber};

use crate::logging::log_filter::LogFilter;
use crate::logging::log_line::LogLine;
use crate::logging::log_ring::LogRing;
use crate::{ErrorChain, env};

/// Takes `tracing` events and `log` records alike, the `log` records of palantir and wgpu among
/// them: writes those its filter passes as lines to its output, and keeps every warning and error
/// in its ring. Keeps no span: it refuses them all, as nothing in the workspace opens one and the
/// spans of `ort` and `rmcp` carry nothing the lines show.
#[derive(Clone, Debug)]
pub struct Logger {
    output: Option<Output>,
    ring: Option<LogRing>,
}

/// Where a [`Logger`] writes its lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogOutput {
    /// The process's stderr, colored when it is a terminal and `NO_COLOR` is unset or empty
    /// (<https://no-color.org>). Stderr in every mode, as MCP's stdio transport forbids any byte
    /// on stdout but its messages and allows logs on stderr
    /// (<https://modelcontextprotocol.io/specification/2025-06-18/basic/transports>).
    Stderr,
    /// The test harness's output, through `eprint!`, which libtest captures for the test whose
    /// thread logs.
    #[cfg(feature = "internals")]
    Test,
}

#[derive(Clone, Debug)]
struct Output {
    filter: LogFilter,
    to: LogOutput,
    color: bool,
}

impl Logger {
    /// Installs a logger as the process's `tracing` subscriber and `log` logger, filtered by
    /// `RUST_LOG`, or by `fallback` when it is unset or names no directive, writing to `to` and
    /// filling `ring`. A `RUST_LOG` the filter cannot read is reported once the logger is in, and
    /// `fallback` filters in its place. A second install leaves the first in place.
    ///
    /// # Panics
    ///
    /// Panics if `fallback` is not a filter, which is a constant at each call.
    pub fn install_from_env(fallback: &str, to: LogOutput, ring: Option<LogRing>) {
        let from_env = LogFilter::from_env_value(env::var_os("RUST_LOG"));
        let filter = match &from_env {
            Ok(Some(filter)) => filter.clone(),
            Ok(None) | Err(_) => LogFilter::parse(fallback).expect("the fallback filter reads"),
        };
        let logger = Self {
            output: Some(Output::new(filter, to)),
            ring,
        };
        let max_level = logger.max_level();
        if subscriber::set_global_default(logger.clone()).is_err() {
            return;
        }
        // `log` holds its logger for the rest of the process, as it holds every logger.
        if log::set_logger(Box::leak(Box::new(logger))).is_ok() {
            log::set_max_level(log_level_filter(max_level));
        }
        if let Err(error) = from_env {
            tracing::warn!(
                "RUST_LOG is ignored and '{fallback}' filters in its place: {}",
                ErrorChain::new(&error)
            );
        }
    }

    pub(super) const fn ring_only(ring: LogRing) -> Self {
        Self {
            output: None,
            ring: Some(ring),
        }
    }

    fn takes(&self, level: Level, target: &str) -> bool {
        self.output
            .as_ref()
            .is_some_and(|output| output.filter.enables(level, target))
            || (self.ring.is_some() && level <= Level::WARN)
    }

    fn max_level(&self) -> LevelFilter {
        let output = self
            .output
            .as_ref()
            .map_or(LevelFilter::OFF, |output| output.filter.max());
        let ring = if self.ring.is_some() {
            LevelFilter::WARN
        } else {
            LevelFilter::OFF
        };
        output.max(ring)
    }

    fn write(&self, level: Level, target: &str, line: &LogLine) {
        if let Some(output) = &self.output
            && output.filter.enables(level, target)
        {
            output.write(level, target, line);
        }
        if let Some(ring) = &self.ring
            && level <= Level::WARN
        {
            ring.push(line.ring_text(level, target));
        }
    }
}

impl Output {
    fn new(filter: LogFilter, to: LogOutput) -> Self {
        let color = match to {
            LogOutput::Stderr => {
                io::stderr().is_terminal()
                    && env::var_os("NO_COLOR").is_none_or(|value| value.is_empty())
            }
            #[cfg(feature = "internals")]
            LogOutput::Test => false,
        };
        Self { filter, to, color }
    }

    /// A clock before 1970 writes the epoch's time.
    fn write(&self, level: Level, target: &str, line: &LogLine) {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let text = line.output_text(since_epoch, level, target, self.color);
        match self.to {
            LogOutput::Stderr => {
                io::stderr().lock().write_all(text.as_bytes()).ok();
            }
            #[cfg(feature = "internals")]
            LogOutput::Test => {
                #[expect(
                    clippy::print_stderr,
                    reason = "libtest captures a test's output through the print macros alone"
                )]
                {
                    eprint!("{text}");
                }
            }
        }
    }
}

impl Subscriber for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.is_event() && self.takes(*metadata.level(), metadata.target())
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(self.max_level())
    }

    /// Never called: [`Self::enabled`] refuses every span.
    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let metadata = event.metadata();
        if self.takes(*metadata.level(), metadata.target()) {
            self.write(
                *metadata.level(),
                metadata.target(),
                &LogLine::of_event(event),
            );
        }
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        self.takes(tracing_level(metadata.level()), metadata.target())
    }

    fn log(&self, record: &log::Record<'_>) {
        let level = tracing_level(record.level());
        if self.takes(level, record.target()) {
            self.write(level, record.target(), &LogLine::of_message(record.args()));
        }
    }

    fn flush(&self) {}
}

const fn tracing_level(level: log::Level) -> Level {
    match level {
        log::Level::Error => Level::ERROR,
        log::Level::Warn => Level::WARN,
        log::Level::Info => Level::INFO,
        log::Level::Debug => Level::DEBUG,
        log::Level::Trace => Level::TRACE,
    }
}

const fn log_level_filter(level: LevelFilter) -> log::LevelFilter {
    match level.into_level() {
        None => log::LevelFilter::Off,
        Some(Level::ERROR) => log::LevelFilter::Error,
        Some(Level::WARN) => log::LevelFilter::Warn,
        Some(Level::INFO) => log::LevelFilter::Info,
        Some(Level::DEBUG) => log::LevelFilter::Debug,
        Some(Level::TRACE) => log::LevelFilter::Trace,
    }
}

#[cfg(test)]
mod tests {
    use log::Log as _;
    use tracing::Level;
    use tracing::level_filters::LevelFilter;

    use crate::logging::log_filter::LogFilter;
    use crate::logging::log_ring::LogRing;
    use crate::logging::logger::{LogOutput, Logger, Output, log_level_filter, tracing_level};

    /// A ring takes warnings and errors whatever the output's filter, and the output what its
    /// filter passes; the most verbose of the two is the hint. A `log` record reaches the ring
    /// as a `tracing` event does, and each level maps to its namesake both ways.
    #[test]
    fn a_record_goes_where_its_level_is_taken() {
        let ring = LogRing::default();
        let quiet = Logger {
            output: Some(Output::new(
                LogFilter::parse("error").unwrap(),
                LogOutput::Test,
            )),
            ring: Some(ring.clone()),
        };
        assert!(quiet.takes(Level::WARN, "lens"));
        assert!(!quiet.takes(Level::INFO, "lens"));
        assert_eq!(quiet.max_level(), LevelFilter::WARN);
        let verbose = Logger {
            output: Some(Output::new(
                LogFilter::parse("lens=debug").unwrap(),
                LogOutput::Test,
            )),
            ring: None,
        };
        assert!(verbose.takes(Level::DEBUG, "lens::random"));
        assert!(!verbose.takes(Level::ERROR, "wgpu"));
        assert_eq!(verbose.max_level(), LevelFilter::DEBUG);
        assert_eq!(ring.logger().max_level(), LevelFilter::WARN);

        let logger = ring.logger();
        let warn = log::Metadata::builder()
            .level(log::Level::Warn)
            .target("wgpu_core")
            .build();
        assert!(logger.enabled(&warn));
        let info = log::Metadata::builder()
            .level(log::Level::Info)
            .target("wgpu_core")
            .build();
        assert!(!logger.enabled(&info));
        logger.log(
            &log::Record::builder()
                .args(format_args!("{} adapters", 2))
                .level(log::Level::Warn)
                .target("wgpu_core::instance")
                .build(),
        );
        logger.log(
            &log::Record::builder()
                .args(format_args!("skipped"))
                .level(log::Level::Info)
                .target("wgpu_core")
                .build(),
        );
        assert_eq!(ring.drain(), ["WARN wgpu_core::instance: 2 adapters"]);

        let levels = [
            (log::Level::Error, Level::ERROR, log::LevelFilter::Error),
            (log::Level::Warn, Level::WARN, log::LevelFilter::Warn),
            (log::Level::Info, Level::INFO, log::LevelFilter::Info),
            (log::Level::Debug, Level::DEBUG, log::LevelFilter::Debug),
            (log::Level::Trace, Level::TRACE, log::LevelFilter::Trace),
        ];
        for (log_level, level, log_filter) in levels {
            assert_eq!(tracing_level(log_level), level);
            assert_eq!(log_level_filter(LevelFilter::from_level(level)), log_filter);
        }
        assert_eq!(log_level_filter(LevelFilter::OFF), log::LevelFilter::Off);
    }
}
