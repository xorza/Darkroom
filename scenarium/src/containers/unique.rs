//! Insertion-ordered uniqueness over a plain `Vec`.

/// Append what `incoming` adds, in first-seen order, dropping what is already
/// there.
///
/// A linear scan rather than a hash set: every list this serves is one batch of worker messages,
/// a handful of ids, so building a set costs more than the scan it would save.
pub(crate) fn extend<T: PartialEq>(into: &mut Vec<T>, incoming: impl IntoIterator<Item = T>) {
    for item in incoming {
        if !into.contains(&item) {
            into.push(item);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::containers::unique;

    /// What is new comes after what was there, in the order first seen; a repeat, held or within
    /// the incoming, adds nothing.
    #[test]
    fn extend_appends_each_new_item_once_in_first_seen_order() {
        let mut held = vec![3, 1];
        unique::extend(&mut held, [1, 4, 3, 5, 4]);
        assert_eq!(held, [3, 1, 4, 5]);
        unique::extend(&mut held, []);
        assert_eq!(held, [3, 1, 4, 5]);
    }
}
