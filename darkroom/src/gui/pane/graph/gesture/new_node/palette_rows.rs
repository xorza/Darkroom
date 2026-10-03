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
mod tests {
    use scenarium::{FuncId, Library, testing};

    use crate::gui::pane::graph::gesture::new_node::node_palette::PaletteEntry;
    use crate::gui::pane::graph::gesture::new_node::palette_rows::{PaletteRows, name_matches};

    /// Four rows over three categories, deliberately out of order and mixing
    /// case so the fold and the raw-order fallback both have to fire.
    fn library() -> Library {
        let mut library = Library::default();
        for spec in ["Zoom/crop", "blur/Sharpen", "Blur/gaussian", "blur/box"] {
            let (category, name) = spec.split_once('/').unwrap();
            library.add(testing::stub_func(FuncId::unique(), name).category(category));
        }
        library
    }

    /// The kept columns as `(category, row names)`, refiltered by `query_lc`
    /// into one `PaletteRows` the whole test reuses.
    fn shape(
        rows: &mut PaletteRows,
        library: &Library,
        query_lc: &str,
    ) -> Vec<(String, Vec<String>)> {
        rows.refilter(library, library.funcs().map(PaletteEntry::Func), query_lc);
        rows.columns()
            .map(|column| {
                let entries: Vec<_> = column
                    .iter()
                    .map(|row| row.entry(library).unwrap())
                    .collect();
                (
                    entries[0].category().to_owned(),
                    entries.iter().map(|e| e.name().to_owned()).collect(),
                )
            })
            .collect()
    }

    /// The palette groups by sorting, so the kept order *is* the column
    /// layout: categories folded-alphabetically, rows the same inside each,
    /// and every category one contiguous run. Each filtering replaces the
    /// last one whole.
    #[test]
    fn rows_sort_into_one_contiguous_run_per_category() {
        let library = library();
        let mut rows = PaletteRows::default();
        let owned = |columns: &[(&str, &[&str])]| -> Vec<(String, Vec<String>)> {
            columns
                .iter()
                .map(|(category, names)| {
                    (
                        (*category).to_owned(),
                        names.iter().map(|name| (*name).to_owned()).collect(),
                    )
                })
                .collect()
        };

        // "Blur" and "blur" fold alike, so they sort adjacently — and stay
        // two runs, because the fallback orders them by the raw name.
        assert_eq!(
            shape(&mut rows, &library, ""),
            owned(&[
                ("Blur", &["gaussian"]),
                ("blur", &["box", "Sharpen"]),
                ("Zoom", &["crop"]),
            ]),
            "no query lists every row, category-major then name",
        );

        // A query the *category* carries reveals both blur columns whole,
        // including the row whose own name holds no "blur".
        assert_eq!(
            shape(&mut rows, &library, "blur"),
            owned(&[("Blur", &["gaussian"]), ("blur", &["box", "Sharpen"])]),
            "a category match keeps its rows whatever they are named",
        );

        // A query only a row name carries takes that row and drops the rest
        // of its category with it.
        assert_eq!(
            shape(&mut rows, &library, "box"),
            owned(&[("blur", &["box"])]),
            "a name match keeps the row alone",
        );

        assert!(
            shape(&mut rows, &library, "nothing").is_empty(),
            "a query nothing carries lists no column at all",
        );
    }

    #[test]
    fn name_matches_is_case_insensitive_substring_with_empty_query_wildcard() {
        // Empty query is the "show everything" wildcard.
        assert!(name_matches("Gaussian Blur", ""));
        assert!(name_matches("", ""));
        // Case-insensitive substring anywhere in the name. Caller passes an
        // already-lowercased query, so only the name is folded here.
        assert!(name_matches("Gaussian Blur", "blur"));
        assert!(name_matches("Gaussian Blur", "gauss"));
        assert!(name_matches("Gaussian Blur", "an bl"));
        // Non-substring and wrong-fragment queries reject.
        assert!(!name_matches("Gaussian Blur", "sharpen"));
        assert!(!name_matches("Blur", "blurry"));
        // A non-lowercased query never matches a lowercased name — the
        // contract is "query already lowercased", so this documents that a
        // caller who forgets to fold gets no false positives. It holds on
        // both sides of the ASCII fast path.
        assert!(!name_matches("blur", "BLUR"));
        assert!(!name_matches("Grün", "GRÜN"));
        // Non-ASCII names fold by the Unicode rules, not byte-wise.
        assert!(name_matches("Grün", "grün"));
        assert!(name_matches("Ölfilter", "ölfil"));
        assert!(!name_matches("Grün", "grun"));
        // An ASCII name never matches a non-ASCII query.
        assert!(!name_matches("Blur", "blür"));
        // The name folds `char` by `char`, as the query did: a final capital
        // sigma folds to `σ` on both sides, where a whole-string fold would
        // turn the name's into `ς` and miss a query typed in capitals.
        assert!(name_matches("ΟΔΟΣ", "οδοσ"));
        assert!(name_matches("ΜΕΓΑΛΟΣ ΔΡΟΜΟΣ", "λοσ δρ"));
    }
}
