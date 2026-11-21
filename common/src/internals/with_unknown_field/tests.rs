use super::*;

/// A named struct takes the field first, on the line after its opener, the root's and a
/// nested one's alike, past a blank line and a comment too; a tuple, a newtype, a map's key
/// and a list open none.
#[test]
fn each_named_struct_takes_the_field_and_nothing_else_does() {
    let text = "(\n    a: (\n        b: 1,\n    ),\n    c: Some((\n        1,\n    )),\n    d: {\n        (\"k\"): [\n            2,\n        ],\n    },\n    e: (\n\n        // note\n        f: 2,\n    ),\n)\n";
    let variants = WithUnknownField::each_struct(text);
    assert_eq!(
        variants.iter().map(|v| v.line).collect::<Vec<_>>(),
        [2, 3, 14],
        "the root, line 1, `a`, line 2, and `e`, line 13, whose field follows a blank line \
         and a comment, each take the field on the line after"
    );
    assert_eq!(
        variants[1].text,
        "(\n    a: (\nunknown_field: (),\n        b: 1,\n    ),\n    c: Some((\n        1,\n    )),\n    d: {\n        (\"k\"): [\n            2,\n        ],\n    },\n    e: (\n\n        // note\n        f: 2,\n    ),\n)\n"
    );
    assert_eq!(
        variants[0].text.lines().nth(variants[0].line - 1),
        Some("unknown_field: (),")
    );
}

/// A path's `::` and a number name no field, and a comment that ends in `(` opens nothing.
#[test]
fn a_path_or_a_number_names_no_field() {
    assert!(opens_struct("(\n", "    name: 1,\n"));
    assert!(!opens_struct("    // a note (\n", "    name: 1,\n"));
    assert!(!opens_struct("(\n", "    std::x,\n"));
    assert!(!opens_struct("(\n", "    1: 2,\n"));
    assert!(!opens_struct("[\n", "    name: 1,\n"));
}
