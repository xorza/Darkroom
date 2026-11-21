//! [`WithUnknownField`]: a RON text with a field added that no type holds, for a test that a load
//! refuses one wherever it stands.

/// The field each [`WithUnknownField`] adds, which no type of the workspace holds.
pub const UNKNOWN_FIELD: &str = "unknown_field";

/// A RON text with [`UNKNOWN_FIELD`] added as the first field of one struct.
#[derive(Debug)]
pub struct WithUnknownField {
    /// The line the added field stands on, from 1, as a RON error's span counts lines.
    pub line: usize,
    pub text: String,
}

impl WithUnknownField {
    /// `text`, pretty RON, once for each struct with named fields it holds, in the order they
    /// open: a struct opens on a line that ends in `(` and whose next line of code, past blank
    /// lines and comments, names a field, as a pretty writer lays one out. A tuple or a newtype
    /// struct, whose next line names none, takes no field.
    pub fn each_struct(text: &str) -> Vec<Self> {
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        (0..lines.len())
            .filter(|&idx| {
                let next = lines[idx + 1..].iter().find(|line| {
                    let line = line.trim_start();
                    !line.is_empty() && !line.starts_with("//")
                });
                next.is_some_and(|next| opens_struct(lines[idx], next))
            })
            .map(|idx| {
                let (head, tail) = lines.split_at(idx + 1);
                Self {
                    line: idx + 2,
                    text: format!("{}{UNKNOWN_FIELD}: (),\n{}", head.concat(), tail.concat()),
                }
            })
            .collect()
    }
}

/// Whether `line` opens a struct whose first field `next` names: the line is code that ends in
/// `(`, and `next` starts with an identifier and a `:` that is no path's `::`.
fn opens_struct(line: &str, next: &str) -> bool {
    if line.trim_start().starts_with("//") || !line.trim_end().ends_with('(') {
        return false;
    }
    let next = next.trim_start();
    let ident_len = next
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(next.len());
    let rest = &next[ident_len..];
    ident_len > 0
        && !next.starts_with(|c: char| c.is_ascii_digit())
        && rest.starts_with(':')
        && !rest.starts_with("::")
}

#[cfg(test)]
mod tests;
