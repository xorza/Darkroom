//! [`PaletteRows`]: the rows the palette's query matched, kept between the
//! frames that record them.

use std::cmp::Ordering;
use std::ops::Range;

use scenarium::{FuncId, Library, SpecialNode};

use crate::gui::pane::graph::gesture::new_node::node_palette::PaletteEntry;

/// One palette row by identity, so the list outlives the borrow of the
/// library that filled it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PaletteRow {
    Func(FuncId),
    Special(SpecialNode),
}

impl PaletteRow {
    /// The row's entry, or `None` for a func the library no longer holds.
    pub(super) fn entry(self, library: &Library) -> Option<PaletteEntry<'_>> {
        match self {
            Self::Func(id) => library.by_id(id).map(PaletteEntry::Func),
            Self::Special(special) => Some(PaletteEntry::Special(special)),
        }
    }
}

/// The rows the palette's query matched, category-major then name, and each
/// category's run of them.
///
/// Filtered and sorted only when the palette opens or its query changes; the
/// frames between record straight off the kept rows, one library lookup per
/// row and no allocation. Both buffers keep their capacity across filterings.
#[derive(Debug, Default)]
pub(super) struct PaletteRows {
    rows: Vec<PaletteRow>,
    /// One range of `rows` per category, in order.
    columns: Vec<Range<usize>>,
}

impl PaletteRows {
    /// Keep the `entries` that `query_lc` matches, category-major then name,
    /// so a category's rows are one contiguous run.
    ///
    /// A matching *category* name reveals that whole column; otherwise a row
    /// is filtered by its own name.
    pub(super) fn refilter<'a>(
        &mut self,
        library: &'a Library,
        entries: impl Iterator<Item = PaletteEntry<'a>>,
        query_lc: &str,
    ) {
        self.rows.clear();
        self.columns.clear();
        self.rows.extend(
            entries
                .filter(|entry| {
                    name_matches(entry.category(), query_lc) || name_matches(entry.name(), query_lc)
                })
                .map(PaletteEntry::row),
        );
        let entry = |row: &PaletteRow| {
            row.entry(library)
                .expect("a row just filtered from the library resolves in it")
        };
        // Case-insensitive by comparison, not by key: a folded key per row
        // would allocate one `String` each, for a question `char`-wise
        // folding answers in place. The raw-order fallback keeps two
        // categories that fold alike in runs of their own.
        self.rows.sort_by(|a, b| {
            let (a, b) = (entry(a), entry(b));
            lowercase_cmp(a.category(), b.category())
                .then_with(|| lowercase_cmp(a.name(), b.name()))
        });
        let mut start = 0;
        for end in 1..=self.rows.len() {
            if end == self.rows.len()
                || entry(&self.rows[end]).category() != entry(&self.rows[start]).category()
            {
                self.columns.push(start..end);
                start = end;
            }
        }
    }

    /// Each category's rows, in order.
    pub(super) fn columns(&self) -> impl Iterator<Item = &[PaletteRow]> {
        self.columns.iter().map(|run| &self.rows[run.clone()])
    }
}

/// Case-insensitive ordering of two palette row names, without materializing
/// a folded copy of either. Falls back to the raw order for names that fold
/// to the same thing, so the sort stays total.
fn lowercase_cmp(a: &str, b: &str) -> Ordering {
    let folded = a
        .chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase));
    folded.then_with(|| a.cmp(b))
}

/// Case-insensitive substring match used by the palette search. An empty
/// (already-lowercased) query matches everything.
///
/// Folds `char` by `char`, as the query was, so both sides fold by one rule
/// and nothing is allocated; ASCII names — every built-in one — compare
/// bytes.
fn name_matches(name: &str, query_lc: &str) -> bool {
    if query_lc.is_empty() {
        return true;
    }
    if !name.is_ascii() {
        return name.char_indices().any(|(start, _)| {
            let mut folded = name[start..].chars().flat_map(char::to_lowercase);
            query_lc.chars().all(|wanted| folded.next() == Some(wanted))
        });
    }
    // Only the name folds. Folding the query here too would quietly accept a
    // caller that forgot to, and the non-ASCII branch above can't.
    let (name, query) = (name.as_bytes(), query_lc.as_bytes());
    name.windows(query.len()).any(|window| {
        window
            .iter()
            .zip(query)
            .all(|(byte, folded)| byte.to_ascii_lowercase() == *folded)
    })
}

#[cfg(test)]
mod tests;
