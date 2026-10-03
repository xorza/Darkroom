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
fn shape(rows: &mut PaletteRows, library: &Library, query_lc: &str) -> Vec<(String, Vec<String>)> {
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
