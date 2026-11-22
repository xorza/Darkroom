//! [`LogFilter`]: which targets log at which levels.

use std::cmp::Reverse;
use std::ffi::OsString;

use tracing::Level;
use tracing::level_filters::LevelFilter;

/// `RUST_LOG`'s comma-separated directives, `level`, `target` or `target=level`, read as
/// `tracing-subscriber`'s `EnvFilter` and `env_logger` read them: a target matches every target
/// it begins, so `wgpu=warn` holds `wgpu_core` too; the longest matching directive decides, a
/// later one replacing an earlier of its target; a bare target logs at every level, and a target
/// no directive names logs at the bare level, or not at all without one. Refused: `EnvFilter`'s
/// span and field directives, as nothing in the workspace opens a span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LogFilter {
    default: LevelFilter,
    /// Longest target first, each target once.
    directives: Vec<Directive>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Directive {
    target: String,
    level: LevelFilter,
}

/// A `RUST_LOG` the filter cannot read.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(super) enum LogFilterError {
    #[error("RUST_LOG is not text")]
    NotText,
    #[error("no target before '=' in the directive '{0}'")]
    NoTarget(String),
    #[error("no level named in the directive '{0}'")]
    Level(String),
    #[error("the directive '{0}' filters spans or fields, which this logger does not")]
    Span(String),
}

impl LogFilter {
    /// Logs nothing.
    pub(super) const OFF: Self = Self {
        default: LevelFilter::OFF,
        directives: Vec::new(),
    };

    pub(super) fn parse(text: &str) -> Result<Self, LogFilterError> {
        let mut filter = Self::OFF;
        for directive in text.split(',').map(str::trim).filter(|d| !d.is_empty()) {
            if directive.contains(['[', '{']) {
                return Err(LogFilterError::Span(directive.to_owned()));
            }
            let (target, level) = match directive.split_once('=') {
                Some((target, level)) => {
                    // tracing reads an empty level as `error`; here it names none.
                    let level = Some(level.trim())
                        .filter(|level| !level.is_empty())
                        .and_then(|level| level.parse().ok())
                        .ok_or_else(|| LogFilterError::Level(directive.to_owned()))?;
                    (target.trim(), level)
                }
                None => match directive.parse() {
                    Ok(level) => {
                        filter.default = level;
                        continue;
                    }
                    Err(_) => (directive, LevelFilter::TRACE),
                },
            };
            if target.is_empty() {
                return Err(LogFilterError::NoTarget(directive.to_owned()));
            }
            filter.directives.retain(|d| d.target != target);
            filter.directives.push(Directive {
                target: target.to_owned(),
                level,
            });
        }
        filter.directives.sort_by_key(|d| Reverse(d.target.len()));
        Ok(filter)
    }

    /// The filter of `RUST_LOG`'s `value`; `None` when it is unset or names no directive.
    pub(super) fn from_env_value(value: Option<OsString>) -> Result<Option<Self>, LogFilterError> {
        let Some(text) = value else {
            return Ok(None);
        };
        let text = text.into_string().map_err(|_| LogFilterError::NotText)?;
        if text.split(',').all(|d| d.trim().is_empty()) {
            return Ok(None);
        }
        Self::parse(&text).map(Some)
    }

    pub(super) fn enables(&self, level: Level, target: &str) -> bool {
        let filter = self
            .directives
            .iter()
            .find(|d| target.starts_with(d.target.as_str()))
            .map_or(self.default, |d| d.level);
        level <= filter
    }

    /// The most verbose level any target logs at.
    pub(super) fn max(&self) -> LevelFilter {
        self.directives
            .iter()
            .map(|d| d.level)
            .fold(self.default, Ord::max)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use tracing::Level;
    use tracing::level_filters::LevelFilter;

    use crate::logging::log_filter::{LogFilter, LogFilterError};

    /// `info,rmcp=warn`: `rmcp` and its modules from warn, the rest from info. A target matches
    /// every target it begins (`wgpu` holds `wgpu_core`), the longest directive decides, and a
    /// later directive replaces an earlier of its target; a bare target logs at every level and
    /// leaves the rest off; names read in any case.
    #[test]
    fn the_longest_matching_directive_decides() {
        let filter = LogFilter::parse("info, rmcp=warn").unwrap();
        assert!(filter.enables(Level::INFO, "darkroom::gui"));
        assert!(!filter.enables(Level::DEBUG, "darkroom::gui"));
        assert!(filter.enables(Level::WARN, "rmcp::service"));
        assert!(!filter.enables(Level::INFO, "rmcp::service"));
        assert_eq!(filter.max(), LevelFilter::INFO);

        let filter = LogFilter::parse("wgpu=warn,wgpu_core=error,wgpu=DEBUG").unwrap();
        assert!(filter.enables(Level::DEBUG, "wgpu_hal::metal"));
        assert!(!filter.enables(Level::WARN, "wgpu_core::device"));
        assert!(filter.enables(Level::ERROR, "wgpu_core::device"));
        assert!(!filter.enables(Level::ERROR, "lumos"));
        assert_eq!(filter.max(), LevelFilter::DEBUG);

        let filter = LogFilter::parse("lumos").unwrap();
        assert!(filter.enables(Level::TRACE, "lumos::stacking"));
        assert!(!filter.enables(Level::ERROR, "darkroom"));
        assert_eq!(filter.max(), LevelFilter::TRACE);

        let filter = LogFilter::parse("debug,lumos=off").unwrap();
        assert!(!filter.enables(Level::ERROR, "lumos"));
        assert!(filter.enables(Level::DEBUG, "darkroom"));

        assert!(!LogFilter::OFF.enables(Level::ERROR, "darkroom"));
        assert_eq!(LogFilter::OFF.max(), LevelFilter::OFF);
        assert_eq!(LogFilter::parse(" , ").unwrap(), LogFilter::OFF);
    }

    /// An unset `RUST_LOG` and one of no directive leave the fallback to filter; a set one is
    /// read.
    #[test]
    fn rust_log_filters_when_it_names_a_directive() {
        assert_eq!(LogFilter::from_env_value(None), Ok(None));
        assert_eq!(
            LogFilter::from_env_value(Some(OsString::from(" ,"))),
            Ok(None)
        );
        assert_eq!(
            LogFilter::from_env_value(Some(OsString::from("warn"))),
            Ok(Some(LogFilter::parse("warn").unwrap()))
        );
        assert_eq!(
            LogFilter::from_env_value(Some(OsString::from("lumos=loud"))),
            Err(LogFilterError::Level("lumos=loud".to_owned()))
        );
    }

    /// A level no name reads, an empty level, a span or field directive and a level with no
    /// target are refused, each naming its directive.
    #[test]
    fn an_unreadable_directive_is_refused() {
        assert_eq!(
            LogFilter::parse("lumos="),
            Err(LogFilterError::Level("lumos=".to_owned()))
        );
        assert_eq!(
            LogFilter::parse("info,lumos=loud"),
            Err(LogFilterError::Level("lumos=loud".to_owned()))
        );
        assert_eq!(
            LogFilter::parse("lumos[stack]=debug"),
            Err(LogFilterError::Span("lumos[stack]=debug".to_owned()))
        );
        assert_eq!(
            LogFilter::parse("=debug"),
            Err(LogFilterError::NoTarget("=debug".to_owned()))
        );
    }
}
