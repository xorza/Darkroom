//! Insertion-ordered uniqueness over a plain `Vec`.

/// Append what `incoming` adds, in first-seen order, dropping what is already
/// there.
///
/// A linear scan rather than a hash set: every list this serves is one burst of
/// worker messages, and each message contributes a single id — so building a
/// set costs more than the scan it would save, and the order the host asked in
/// survives.
pub(crate) fn extend<T: PartialEq>(into: &mut Vec<T>, incoming: impl IntoIterator<Item = T>) {
    for item in incoming {
        if !into.contains(&item) {
            into.push(item);
        }
    }
}
