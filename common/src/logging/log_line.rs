//! [`LogLine`]: one record's text, and the lines the logger writes from it.

use std::error::Error;
use std::fmt::{self, Write as _};
use std::time::Duration;

use tracing::field::{Field, Visit};
use tracing::{Event, Level};

use crate::ErrorChain;

const SECS_PER_DAY: u64 = 86_400;

/// One record's `message`, then its other fields as `name=value`, as `tracing-subscriber`'s `fmt`
/// writes them: a field's value by `Debug`, which quotes a string, and an error by its chain.
#[derive(Debug, Default)]
pub(super) struct LogLine {
    message: String,
    fields: String,
}

impl LogLine {
    pub(super) fn of_event(event: &Event<'_>) -> Self {
        let mut line = Self::default();
        event.record(&mut line);
        line
    }

    pub(super) fn of_message(message: &fmt::Arguments<'_>) -> Self {
        Self {
            message: message.to_string(),
            fields: String::new(),
        }
    }

    /// `LEVEL target: message fields`, as the ring keeps it.
    pub(super) fn ring_text(&self, level: Level, target: &str) -> String {
        let mut out = format!("{level} {target}:");
        self.write_text(&mut out);
        out
    }

    /// `timestamp LEVEL target: message fields` and a newline, as the output takes it: the time
    /// in RFC 3339 UTC to the microsecond, the level right-aligned to five columns, and with
    /// `color` the time and target dimmed and the level in its color, as `fmt` writes a line.
    pub(super) fn output_text(
        &self,
        since_epoch: Duration,
        level: Level,
        target: &str,
        color: bool,
    ) -> String {
        const DIM: &str = "\x1b[2m";
        const RESET: &str = "\x1b[0m";
        let (dim, level_color, reset) = if color {
            let level_color = match level {
                Level::TRACE => "\x1b[35m",
                Level::DEBUG => "\x1b[34m",
                Level::INFO => "\x1b[32m",
                Level::WARN => "\x1b[33m",
                Level::ERROR => "\x1b[31m",
            };
            (DIM, level_color, RESET)
        } else {
            ("", "", "")
        };
        let mut out = dim.to_owned();
        write_timestamp(&mut out, since_epoch);
        write!(
            out,
            "{reset} {level_color}{:>5}{reset} {dim}{target}:{reset}",
            level.as_str()
        )
        .ok();
        self.write_text(&mut out);
        out.push('\n');
        out
    }

    /// The message and fields, each after a space.
    fn write_text(&self, out: &mut String) {
        for part in [&self.message, &self.fields] {
            if !part.is_empty() {
                out.push(' ');
                out.push_str(part);
            }
        }
    }
}

impl Visit for LogLine {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            self.record_debug(field, &value);
        }
    }

    fn record_error(&mut self, field: &Field, value: &(dyn Error + 'static)) {
        self.record_debug(field, &format_args!("{}", ErrorChain::new(value)));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            write!(self.message, "{value:?}").ok();
        } else {
            if !self.fields.is_empty() {
                self.fields.push(' ');
            }
            write!(self.fields, "{}={value:?}", field.name()).ok();
        }
    }
}

/// `since_epoch` as `YYYY-MM-DDThh:mm:ss.uuuuuuZ`, its date by Hinnant's `civil_from_days`
/// (<https://howardhinnant.github.io/date_algorithms.html#civil_from_days>) over days since
/// 1970-01-01, a proleptic Gregorian calendar of 400-year eras of 146 097 days.
fn write_timestamp(out: &mut String, since_epoch: Duration) {
    let secs = since_epoch.as_secs();
    let days = secs / SECS_PER_DAY + 719_468;
    let era = days / 146_097;
    let day_of_era = days % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    let secs_of_day = secs % SECS_PER_DAY;
    write!(
        out,
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:06}Z",
        secs_of_day / 3600,
        secs_of_day / 60 % 60,
        secs_of_day % 60,
        since_epoch.subsec_micros()
    )
    .ok();
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tracing::Level;

    use crate::logging::log_line::{LogLine, write_timestamp};

    /// The epoch; 2000-02-29, a leap day of a year divisible by 400, at 951 868 799 s; and
    /// 2026-11-22 21:14:00 at 1 795 382 040 s, each as Python's `datetime` gives it, with the
    /// microseconds a duration's nanoseconds truncate to.
    #[test]
    fn a_timestamp_is_rfc_3339_utc_to_the_microsecond() {
        let cases = [
            (Duration::ZERO, "1970-01-01T00:00:00.000000Z"),
            (
                Duration::new(951_868_799, 999_999_999),
                "2000-02-29T23:59:59.999999Z",
            ),
            (
                Duration::new(1_795_382_040, 123_456_000),
                "2026-11-22T21:14:00.123456Z",
            ),
        ];
        for (since_epoch, text) in cases {
            let mut out = String::new();
            write_timestamp(&mut out, since_epoch);
            assert_eq!(out, text);
        }
    }

    /// The output line pads the level to five columns; with color it dims the time and the
    /// target with its colon and colors the level, and the text between the codes stays the
    /// plain line's; a message alone, fields alone or neither leave no stray space.
    #[test]
    fn an_output_line_is_time_level_target_and_text() {
        let line = LogLine {
            message: "opened".to_owned(),
            fields: "path=\"a.darkroom\"".to_owned(),
        };
        let at = Duration::new(1_795_382_040, 123_456_000);
        assert_eq!(
            line.output_text(at, Level::INFO, "darkroom::io", false),
            "2026-11-22T21:14:00.123456Z  INFO darkroom::io: opened path=\"a.darkroom\"\n"
        );
        assert_eq!(
            line.output_text(at, Level::WARN, "darkroom::io", true),
            "\x1b[2m2026-11-22T21:14:00.123456Z\x1b[0m \x1b[33m WARN\x1b[0m \
             \x1b[2mdarkroom::io:\x1b[0m opened path=\"a.darkroom\"\n"
        );
        assert_eq!(
            line.ring_text(Level::ERROR, "lens"),
            "ERROR lens: opened path=\"a.darkroom\""
        );
        let message = LogLine::of_message(&format_args!("{} frames", 3));
        assert_eq!(
            message.ring_text(Level::WARN, "wgpu"),
            "WARN wgpu: 3 frames"
        );
        let fields = LogLine {
            message: String::new(),
            fields: "port=2".to_owned(),
        };
        assert_eq!(fields.ring_text(Level::WARN, "lens"), "WARN lens: port=2");
        assert_eq!(
            LogLine::default().ring_text(Level::WARN, "lens"),
            "WARN lens:"
        );
    }
}
