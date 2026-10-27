//! [`PaletteRows`]: the rows the palette's query matched, kept between the
//! frames that record them.

use std::cmp::Ordering;
use std::ops::Range;

use scenarium::{FuncId, Library, SpecialNode};

use crate::gui::pane::graph_ui::gesture::new_node_ui::node_palette::PaletteEntry;

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

/// The rows the palette's query matched, in the order they list, and the runs a caption heads.
///
/// With no query, every row, category-major then name, one run per category under its name.
/// With one, the rows ranked by how they match ([`Rank`]), then category and name, as one run
/// under no caption: the best match leads, where the focus starts.
///
/// Filtered and sorted only when the palette opens or its query changes; the frames between
/// record straight off the kept rows, one library lookup per row and no allocation. Both
/// buffers keep their capacity across filterings.
#[derive(Debug, Default)]
pub(super) struct PaletteRows {
    rows: Vec<PaletteRow>,
    /// One range of `rows` per run, in order.
    runs: Vec<Range<usize>>,
    /// Whether each run is a category under its caption: the query was empty.
    grouped: bool,
}

/// How a row matches a query, best first: the order a ranked palette lists in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Rank {
    /// The name starts with the query: "stre" in "Stretch".
    NameStart,
    /// A word of the name starts with it: "stre" in "Auto Stretch".
    WordStart,
    /// The name holds it elsewhere.
    Name,
    /// The category holds it.
    Category,
    /// The description holds it.
    Description,
}

impl Rank {
    /// How `entry` matches `query_lc`, or `None` when it does not.
    pub(super) fn of(entry: PaletteEntry<'_>, query_lc: &str) -> Option<Self> {
        let name = entry.name();
        if folded_starts_with(name, query_lc) {
            Some(Self::NameStart)
        } else if word_starts(name).any(|start| folded_starts_with(&name[start..], query_lc)) {
            Some(Self::WordStart)
        } else if name_matches(name, query_lc) {
            Some(Self::Name)
        } else if name_matches(entry.category(), query_lc) {
            Some(Self::Category)
        } else if entry
            .description()
            .is_some_and(|text| name_matches(text, query_lc))
        {
            Some(Self::Description)
        } else {
            None
        }
    }
}

impl PaletteRows {
    /// Keep the `entries` that `query_lc` matches, in the order the palette lists them.
    pub(super) fn refilter<'a>(
        &mut self,
        library: &'a Library,
        entries: impl Iterator<Item = PaletteEntry<'a>>,
        query_lc: &str,
    ) {
        self.rows.clear();
        self.runs.clear();
        self.grouped = query_lc.is_empty();
        self.rows.extend(
            entries
                .filter(|entry| Rank::of(*entry, query_lc).is_some())
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
            Rank::of(a, query_lc)
                .cmp(&Rank::of(b, query_lc))
                .then_with(|| lowercase_cmp(a.category(), b.category()))
                .then_with(|| lowercase_cmp(a.name(), b.name()))
        });
        if !self.grouped {
            if !self.rows.is_empty() {
                self.runs.push(0..self.rows.len());
            }
            return;
        }
        let mut start = 0;
        for end in 1..=self.rows.len() {
            if end == self.rows.len()
                || entry(&self.rows[end]).category() != entry(&self.rows[start]).category()
            {
                self.runs.push(start..end);
                start = end;
            }
        }
    }

    /// Each run's rows, in order, with the index of its first row in the whole list.
    pub(super) fn runs(&self) -> impl Iterator<Item = (usize, &[PaletteRow])> {
        self.runs
            .iter()
            .map(|run| (run.start, &self.rows[run.clone()]))
    }

    /// Whether each run is a category under its caption.
    pub(super) const fn grouped(&self) -> bool {
        self.grouped
    }

    /// How many rows the list holds.
    pub(super) const fn len(&self) -> usize {
        self.rows.len()
    }

    /// The row at `idx` of the whole list.
    pub(super) fn get(&self, idx: usize) -> Option<PaletteRow> {
        self.rows.get(idx).copied()
    }
}

/// Where each word of `name` starts, past its first: after a space, a slash, a hyphen, an
/// underscore or an opening bracket.
fn word_starts(name: &str) -> impl Iterator<Item = usize> + '_ {
    name.char_indices()
        .zip(name.chars().skip(1))
        .filter(|((_, sep), _)| matches!(sep, ' ' | '/' | '-' | '_' | '('))
        .map(|((at, sep), _)| at + sep.len_utf8())
}

/// Whether `name`, folded `char` by `char`, starts with the already folded `query_lc`.
fn folded_starts_with(name: &str, query_lc: &str) -> bool {
    let mut folded = name.chars().flat_map(char::to_lowercase);
    query_lc.chars().all(|wanted| folded.next() == Some(wanted))
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
