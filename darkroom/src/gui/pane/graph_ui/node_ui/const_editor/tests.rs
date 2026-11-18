use super::*;
use common::Libm;
use glam::{UVec2, Vec2};
use palantir::ColorScheme;
use palantir::Key;

use palantir::Harness;

use crate::gui::theme::Theme;

/// An input with nothing in it shows [`UNSET`] and takes no click: a click on it focuses
/// nothing, and the row around it gets the press.
#[test]
fn an_unset_field_shows_its_mark_and_takes_no_click() {
    let theme = Theme::new(ColorScheme::Dark);
    let id = WidgetId::from_hash("const_editor::unset");
    let field = Field {
        theme: &theme.const_editor.rest,
        id,
    };
    let mut h = Harness::builder(UVec2::new(200, 60)).mono().build();
    let mut record = |ui: &mut Ui| {
        assert!(field.unset(ui, &ConstValue::Null).is_none());
    };
    h.frame(&mut record);
    let shown: Vec<String> = h.widgets(None).into_iter().map(|w| w.text).collect();
    assert!(
        !shown.iter().any(|text| text == UNSET),
        "nothing interactive reads {UNSET}: {shown:?}"
    );
    let center = h.rect(id).expect("the field is arranged").center();
    h.click(center);
    h.frame(&mut record);
    assert_eq!(h.ui().focus(), None, "the click focuses nothing");
}

/// A constant of a type the library does not register, an enum's among them, shows as written
/// and edits nothing: no picker over variants the library cannot name, and a click focuses
/// nothing.
#[test]
fn an_unknown_types_constant_shows_as_written_and_takes_no_edit() {
    use scenarium::EnumId;

    let theme = Theme::new(ColorScheme::Dark);
    let library = Library::default();
    let id = WidgetId::from_hash("const_editor::unknown");
    let value = ConstValue::Enum("Bicubic".to_string());
    let mut h = Harness::builder(UVec2::new(200, 60)).mono().build();
    let mut record = |ui: &mut Ui| {
        let editor = ConstEditor {
            field: Field {
                theme: &theme.const_editor.rest,
                id,
            },
            library: &library,
            value: &value,
            data_type: DataType::Enum(EnumId::from_u128(78)),
            domain: &ValueDomain::Any,
        };
        assert!(editor.show(ui).is_none());
    };
    h.frame(&mut record);
    let center = h.rect(id).expect("the field is arranged").center();
    h.click(center);
    h.frame(&mut record);
    assert_eq!(h.ui().focus(), None, "the click focuses nothing");
}

/// The text field commits through palantir's buffered `TextEdit`: Escape commits nothing and
/// the next session starts at the document value, so Enter there commits nothing; Enter and a
/// click elsewhere each commit what was typed, once, and a blur that follows Enter with no edit
/// between commits nothing more.
///
/// Commits are collected from every record pass, not only the input-observing one: a frame
/// can run a second pass, and the cancelled draft must not commit there.
#[test]
fn escape_drops_the_draft_and_enter_or_blur_commits() {
    #[derive(Debug)]
    struct Probe<'a> {
        field: Field<'a>,
    }
    impl Probe<'_> {
        /// Every commit any pass of a frame produced. The field fills a hugging host, so it
        /// takes its floor and leaves the harness's right side free to click on.
        fn frame(&self, h: &mut Harness) -> Vec<String> {
            let mut commits = Vec::new();
            h.frame(|ui| {
                Panel::hstack()
                    .id_salt("host")
                    .show(ui, |ui| commits.extend(self.field.text(ui, "42")));
            });
            commits
        }

        fn focus(&self, h: &mut Harness) {
            let center = h.rect(self.field.id).expect("field arranged").center();
            h.click(center);
            assert!(self.frame(h).is_empty(), "focusing commits nothing");
        }

        fn focus_and_type(&self, h: &mut Harness) {
            self.focus(h);
            h.key(Key::Char('7'));
            assert!(self.frame(h).is_empty(), "typing commits nothing");
        }
    }

    let theme = Theme::new(ColorScheme::Dark);
    let field = Probe {
        field: Field {
            theme: &theme.const_editor.rest,
            id: WidgetId::from_hash("const_editor::commit_rules"),
        },
    };
    let mut h = Harness::builder(UVec2::new(200, 60)).mono().build();
    field.frame(&mut h);

    field.focus_and_type(&mut h);
    h.key(Key::Escape);
    assert_eq!(
        field.frame(&mut h),
        Vec::<String>::new(),
        "Escape must not commit"
    );
    for later in 0..3 {
        assert!(
            field.frame(&mut h).is_empty(),
            "frame {later} after Escape must not commit"
        );
    }
    field.focus(&mut h);
    h.key(Key::Enter);
    assert!(
        field.frame(&mut h).is_empty(),
        "the next session starts at the document value"
    );

    let elsewhere = Vec2::new(190.0, 55.0);
    field.focus_and_type(&mut h);
    h.key(Key::Enter);
    assert_eq!(field.frame(&mut h).len(), 1, "Enter commits the draft once");
    h.click(elsewhere);
    let commits: Vec<String> = (0..2).flat_map(|_| field.frame(&mut h)).collect();
    assert!(
        commits.is_empty(),
        "a blur with no edit since Enter commits nothing: {commits:?}"
    );

    field.focus_and_type(&mut h);
    h.click(elsewhere);
    let commits: Vec<String> = (0..2).flat_map(|_| field.frame(&mut h)).collect();
    assert_eq!(commits.len(), 1, "a click elsewhere commits the draft once");
}

#[test]
fn parse_any_infers_tightest_kind() {
    // Integers before floats: a bare integer is `Int`, not `Float`; the
    // sign is accepted by `i64::from_str`.
    assert_eq!(parse_any("42"), ConstValue::Int(42));
    assert_eq!(parse_any("-7"), ConstValue::Int(-7));
    assert_eq!(parse_any("+3"), ConstValue::Int(3));
    // Decimals / scientific / leading-or-trailing-dot fall through to float.
    assert_eq!(parse_any("2.5"), ConstValue::Float(2.5));
    assert_eq!(parse_any(".5"), ConstValue::Float(0.5));
    assert_eq!(parse_any("5."), ConstValue::Float(5.0));
    assert_eq!(parse_any("1e3"), ConstValue::Float(1000.0));
    // `true`/`false` in any casing are bools; a non-bool word stays text.
    assert_eq!(parse_any("true"), ConstValue::Bool(true));
    assert_eq!(parse_any("false"), ConstValue::Bool(false));
    assert_eq!(parse_any("True"), ConstValue::Bool(true));
    assert_eq!(parse_any("TRUE"), ConstValue::Bool(true));
    assert_eq!(parse_any("False"), ConstValue::Bool(false));
    assert_eq!(parse_any("yes"), ConstValue::String("yes".into()));
    // Non-finite floats parse as `f64` but are rejected — they stay text
    // rather than becoming `Float(inf)`/`Float(nan)`.
    assert_eq!(parse_any("inf"), ConstValue::String("inf".into()));
    assert_eq!(parse_any("nan"), ConstValue::String("nan".into()));
    // Empty, and numeric-with-suffix, are plain strings.
    assert_eq!(parse_any(""), ConstValue::String(String::new()));
    assert_eq!(parse_any("hello"), ConstValue::String("hello".into()));
    assert_eq!(parse_any("42x"), ConstValue::String("42x".into()));
}

/// [`AnyText`] as a `String` — the shape the assertions read.
fn formatted_any(value: &ConstValue) -> String {
    AnyText(value).to_string()
}

#[test]
fn format_any_round_trips_non_string_kinds() {
    // Int / Float / Bool survive a format→parse round-trip unchanged, so the
    // editor can reformat on blur without flipping the kind. `Float` keeps
    // its `.0` so a whole-number float doesn't collapse back to `Int`.
    for value in [
        ConstValue::Int(42),
        ConstValue::Float(3.0),
        ConstValue::Float(2.5),
        ConstValue::Float(1000.0),
        ConstValue::Bool(true),
        ConstValue::Bool(false),
    ] {
        assert_eq!(
            parse_any(&formatted_any(&value)),
            value,
            "round-trip {value:?}"
        );
    }
    assert_eq!(formatted_any(&ConstValue::Float(3.0)), "3.0");
    // `Null` (an unseeded `Any`) shows blank rather than the text "null".
    assert_eq!(formatted_any(&ConstValue::Null), "");
    // A numeric-looking string is the one kind that doesn't round-trip — it
    // reparses as the number. The accepted ambiguity of an untyped literal.
    assert_eq!(
        parse_any(&formatted_any(&ConstValue::String("42".into()))),
        ConstValue::Int(42)
    );
}

#[test]
fn float_speed_scales_with_magnitude() {
    // Below a unit, speed floors at 0.01/px (max(|v|, 1) * 0.01).
    assert_eq!(float_speed(0.0), 0.01);
    assert_eq!(float_speed(0.5), 0.01);
    assert_eq!(float_speed(-0.5), 0.01);
    // Above a unit it scales proportionally: 50 → 0.5/px, 1000 → 10/px.
    assert_eq!(float_speed(50.0), 0.5);
    assert_eq!(float_speed(1000.0), 10.0);
    // Uses the magnitude, so sign doesn't matter.
    assert_eq!(float_speed(-1000.0), 10.0);
    // A big value really does scrub faster than a small one per pixel.
    assert!(float_speed(1000.0) > float_speed(1.0));
}

#[test]
fn int_speed_floors_then_scales() {
    // Small counts floor at 0.25/px (~4 px per whole step).
    assert_eq!(int_speed(0), 0.25);
    assert_eq!(int_speed(5), 0.25);
    assert_eq!(int_speed(-5), 0.25);
    // Above the floor it scales: 200 → 2/px, 1000 → 10/px.
    assert_eq!(int_speed(200), 2.0);
    assert_eq!(int_speed(1000), 10.0);
    // The most negative count has no positive `i64`: |i64::MIN| = 2^63, and 2^63 · 0.01 as f64.
    assert_eq!(int_speed(i64::MIN), 2_f64.libm_powi(63) * 0.01);
}

/// A commit sets the tightest kind its text reads as, and one that reads back as the value it
/// replaced sets nothing.
#[test]
fn an_any_field_commits_only_a_real_edit() {
    let text = |value: &str| ConstValue::String(value.to_string());
    for (value, committed, set) in [
        (ConstValue::Int(5), "+5", None),
        (ConstValue::Float(3.0), "3", Some(ConstValue::Int(3))),
        (ConstValue::Null, "7", Some(ConstValue::Int(7))),
        (ConstValue::Int(42), "43", Some(ConstValue::Int(43))),
        (text("a"), "b", Some(text("b"))),
        (text("42"), "42.5", Some(ConstValue::Float(42.5))),
    ] {
        assert_eq!(
            any_edit(committed, &value),
            set,
            "{committed:?} over {value:?}"
        );
    }
}

/// A number with a span commits inside it: 1.7 typed into a field of [0, 1] lands 1.0, and -3
/// lands 0.0. The field shows how far along the span its value sits: a fill a quarter of its
/// width at 0.25, none for a span with no ceiling, and one that follows a scrub while it is held.
#[test]
fn a_spanned_number_commits_inside_its_span_and_shows_its_share() {
    use palantir::Modifiers;
    use scenarium::FloatSpan;

    let theme = Theme::new(ColorScheme::Dark);
    let library = Library::default();
    let id = WidgetId::from_hash("const_editor::spanned");
    let span = ValueDomain::Float(FloatSpan::between(0.0, 1.0));
    let mut h = Harness::builder(UVec2::new(200, 60)).mono().build();
    let commit = |h: &mut Harness, domain: &ValueDomain, value: f64, typed: &str| {
        let value = ConstValue::Float(value);
        let mut committed = Vec::new();
        let mut record = |ui: &mut Ui| {
            let editor = ConstEditor {
                field: Field {
                    theme: &theme.const_editor.rest,
                    id,
                },
                library: &library,
                value: &value,
                data_type: DataType::Float,
                domain,
            };
            committed.extend(editor.show(ui));
        };
        h.frame(&mut record);
        let fill = h.rect(id.with("fill")).map(|rect| rect.size.w);
        let center = h.rect(id).expect("the field is arranged").center();
        h.click(center);
        h.frame(&mut record);
        h.hold_modifiers(Modifiers::CTRL);
        h.key(Key::Char('a'));
        h.hold_modifiers(Modifiers::NONE);
        h.type_text(typed);
        h.key(Key::Enter);
        h.frame(&mut record);
        (committed, fill)
    };
    // 0.25 of the field's 200 px, the harness's width, which the field fills.
    let (committed, fill) = commit(&mut h, &span, 0.25, "1.7");
    assert_eq!(committed, [ConstValue::Float(1.0)]);
    assert_eq!(fill, Some(50.0));
    let (committed, _) = commit(&mut h, &span, 0.5, "-3");
    assert_eq!(committed, [ConstValue::Float(0.0)]);
    let unbounded = ValueDomain::Float(FloatSpan::at_least(0.0));
    let (committed, fill) = commit(&mut h, &unbounded, 0.5, "7");
    assert_eq!(
        committed,
        [ConstValue::Float(7.0)],
        "no ceiling to hold it under"
    );
    assert_eq!(fill, None, "no far end to fill toward");

    // A scrub moves the fill while it is held, before the commit moves the document's value:
    // 0.25 dragged 25 px right at 0.01 a pixel, `float_speed`'s 1 % of a unit, reads 0.5, a fill
    // half the field's 200 px. The record keeps the document's 0.25 throughout, as a frame does.
    let value = ConstValue::Float(0.25);
    let mut record = |ui: &mut Ui| {
        let editor = ConstEditor {
            field: Field {
                theme: &theme.const_editor.rest,
                id,
            },
            library: &library,
            value: &value,
            data_type: DataType::Float,
            domain: &span,
        };
        let _ = editor.show(ui);
    };
    h.frame(&mut record);
    let center = h.rect(id).expect("the field is arranged").center();
    let fill = |h: &Harness| h.rect(id.with("fill")).map(|rect| rect.size.w);
    assert_eq!(fill(&h), Some(50.0));
    h.press(center, palantir::PointerButton::Left);
    h.frame(&mut record);
    h.drag_to(center + Vec2::new(25.0, 0.0));
    h.frame(&mut record);
    h.frame(&mut record);
    assert_eq!(fill(&h), Some(100.0), "the fill follows the drag");
    h.release(palantir::PointerButton::Left);
    h.frame(&mut record);
}

/// Every editor in a column takes the column's width, so the boxes of a node's inputs line up on
/// both edges: a picker whose label is wider than the floor widens the column, and a number, a
/// text, an empty field and a value of no registered type fill it beside it; with only narrow
/// values the column is the floor, `ConstEditorTheme::width`.
#[test]
fn every_editor_fills_its_column() {
    use scenarium::ValueVariant;

    let theme = Theme::new(ColorScheme::Dark);
    let library = Library::default();
    let wide = "a picker label wider than the floor";
    let picker = ValueDomain::Variants(vec![
        ValueVariant::new("wide", ConstValue::Int(0)).display(wide),
        ValueVariant::new("narrow", ConstValue::Int(1)),
    ]);
    let ids = ["picker", "float", "text", "empty"].map(WidgetId::from_hash);
    let mut h = Harness::builder(UVec2::new(600, 200)).mono().build();
    let widths = |h: &mut Harness, with_picker: bool| {
        h.frame(&mut |ui: &mut Ui| {
            Panel::vstack().id_salt("column").show(ui, |ui| {
                let field = |id| Field {
                    theme: &theme.const_editor.rest,
                    id,
                };
                let editor = |id, value, data_type, domain| ConstEditor {
                    field: field(id),
                    library: &library,
                    value,
                    data_type,
                    domain,
                };
                if with_picker {
                    let _ = editor(ids[0], &ConstValue::Int(0), DataType::Int, &picker).show(ui);
                }
                let _ = editor(
                    ids[1],
                    &ConstValue::Float(0.5),
                    DataType::Float,
                    &ValueDomain::Any,
                )
                .show(ui);
                let text = ConstValue::String("hi".to_owned());
                let _ = editor(ids[2], &text, DataType::String, &ValueDomain::Any).show(ui);
                field(ids[3]).empty(ui);
            });
        });
        ids.map(|id| h.rect(id).map(|rect| rect.size.w))
    };
    let [picker_w, float_w, text_w, empty_w] = widths(&mut h, true);
    let picker_w = picker_w.expect("the picker is arranged");
    assert!(
        picker_w > theme.const_editor.rest.width,
        "the label widens the column past the floor: {picker_w}"
    );
    assert_eq!(float_w, Some(picker_w));
    assert_eq!(text_w, Some(picker_w));
    assert_eq!(empty_w, Some(picker_w));
    let [picker_w, float_w, text_w, empty_w] = widths(&mut h, false);
    let floor = Some(theme.const_editor.rest.width);
    assert_eq!(picker_w, None);
    assert_eq!(float_w, floor);
    assert_eq!(text_w, floor);
    assert_eq!(empty_w, floor);
}
