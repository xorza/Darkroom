//! Small display formatters shared across the GUI. Every reader of a
//! magnitude or a constant — the status bar, a node's footer and its fields,
//! the inspector, a preview card's info strip — formats through here, so the
//! same value never renders two ways.

pub(crate) mod bytes;
pub(crate) mod elapsed;
pub(crate) mod path_preview;
pub(crate) mod shown_value;

use std::io::Cursor;
use std::str;

/// The integer part `value` prints with at `precision` decimals, read off the formatter itself so a
/// unit decision agrees with the digits the label then shows — including where they round up. A
/// stack buffer, because every caller formats once per frame.
fn printed_integer_part(value: f64, precision: usize) -> u64 {
    use std::io::Write as _;
    let mut buffer = [0u8; 48];
    let mut cursor = Cursor::new(&mut buffer[..]);
    write!(cursor, "{value:.precision$}").expect("48 bytes hold every magnitude formatted here");
    let len = usize::try_from(cursor.position()).expect("the cursor sits inside the buffer");
    let printed = &buffer[..len];
    let integer = printed
        .split(|&byte| byte == b'.')
        .next()
        .unwrap_or(printed);
    str::from_utf8(integer)
        .ok()
        .and_then(|digits| digits.parse().ok())
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
