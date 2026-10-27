use scenarium::{FuncId, Library, internals};

use crate::gui::pane::graph_ui::gesture::new_node_ui::node_palette::PaletteEntry;
use crate::gui::pane::graph_ui::gesture::new_node_ui::palette_rows::{
    PaletteRows, Rank, name_matches,
};

/// Four rows over three categories, deliberately out of order and mixing
/// case so the fold and the raw-order fallback both have to fire.
fn library() -> Library {
    let mut library = Library::default();
    for spec in ["Zoom/crop", "blur/Sharpen", "Blur/gaussian", "blur/box"] {
        let (category, name) = spec.split_once('/').unwrap();
        library.add(
            internals::stub_func(FuncId::unique(), name)
                .category(category)
                .build(),
        );
    }
    library
}

/// The kept runs as `(category of the first row, row names)`, refiltered by `query_lc` into one
/// `PaletteRows` the whole test reuses.
fn shape(rows: &mut PaletteRows, library: &Library, query_lc: &str) -> Vec<(String, Vec<String>)> {
    rows.refilter(library, library.iter().map(PaletteEntry::Func), query_lc);
    rows.runs()
        .map(|(_, run)| {
            let entries: Vec<_> = run.iter().map(|row| row.entry(library).unwrap()).collect();
            (
                entries[0].category().to_owned(),
                entries.iter().map(|e| e.name().to_owned()).collect(),
            )
        })
        .collect()
}

fn owned(runs: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
    runs.iter()
        .map(|(category, names)| {
            (
                (*category).to_owned(),
                names.iter().map(|name| (*name).to_owned()).collect(),
            )
        })
        .collect()
}

/// With no query the palette groups by sorting, so the kept order *is* the layout: categories
/// folded-alphabetically, rows the same inside each, and every category one contiguous run
/// under its caption. With one, the matches are one run with no caption. Each filtering
/// replaces the last one whole.
#[test]
fn no_query_groups_by_category_and_a_query_lists_one_run() {
    let library = library();
    let mut rows = PaletteRows::default();

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
    assert!(rows.grouped());

    // A query only the categories carry keeps their rows whatever they are named, as one run.
    assert_eq!(
        shape(&mut rows, &library, "blur"),
        owned(&[("Blur", &["gaussian", "box", "Sharpen"])]),
        "a category match keeps its rows, category then name",
    );
    assert!(!rows.grouped());
    assert_eq!(
        shape(&mut rows, &library, "box"),
        owned(&[("blur", &["box"])]),
        "a name match keeps the row alone",
    );
    assert!(
        shape(&mut rows, &library, "nothing").is_empty(),
        "a query nothing carries lists no run at all",
    );
    assert_eq!(rows.len(), 0);
}

/// A query ranks the matches: a name that starts with it, then a name a word of which does,
/// then a name that holds it, then a category, then a description; a tie sorts by category,
/// then name. "stre" puts Stream before Auto Stretch before Build Stretch Config, then the
/// Contrast row whose description says "stretch".
#[test]
fn a_query_ranks_name_start_then_word_start_then_name_then_category_then_description() {
    let mut library = Library::default();
    for (name, category, description) in [
        ("Contrast", "Image", "Linear stretch of the levels"),
        ("Build Stretch Config", "Astro", ""),
        ("Auto Stretch", "Astro", ""),
        ("Stream", "System", ""),
        ("Restretch", "Image", ""),
        ("Blur", "Restricted", ""),
    ] {
        let func = internals::stub_func(FuncId::unique(), name).category(category);
        let func = if description.is_empty() {
            func
        } else {
            func.description(description)
        };
        library.add(func.build());
    }
    let mut rows = PaletteRows::default();
    assert_eq!(
        shape(&mut rows, &library, "stre"),
        owned(&[(
            "System",
            &[
                "Stream",
                "Auto Stretch",
                "Build Stretch Config",
                "Restretch",
                "Contrast",
            ]
        )]),
    );
    let ranks: Vec<Rank> = ["Stream", "Auto Stretch", "Restretch", "Blur", "Contrast"]
        .map(|name| {
            let func = library.iter().find(|func| func.name() == name).unwrap();
            Rank::of(PaletteEntry::Func(func), "re").expect("each matches")
        })
        .into();
    assert_eq!(
        ranks,
        [
            Rank::Name,
            Rank::Name,
            Rank::NameStart,
            Rank::Category,
            Rank::Description,
        ],
        "\"re\": inside Stream and Stretch; Restretch starts with it; Restricted is the \
         category's; the levels are the description's"
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
