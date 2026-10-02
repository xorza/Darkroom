use super::*;

use palantir::internals::UiHarness;

use crate::gui::theme::Theme;

/// The unfocused mirror *refills* its retained buffer rather than
/// replacing it, so a literal that never changes must still read as one
/// copy of itself after any number of frames — an append that forgot to
/// clear would show `4242` on the second.
///
/// The same frames must commit nothing: the editor speaks only on the
/// frame its edit lands, which is what keeps the commit's one `String`
/// at gesture rate instead of frame rate.
#[test]
fn the_unfocused_mirror_refills_its_buffer_instead_of_appending() {
    let theme = Theme::default();
    let editor = &theme.const_value_editor.drag_value.editor;
    let id = WidgetId::from_hash("value_editor::mirror");
    let value = ConstValue::Int(42);
    let mut h = UiHarness::new(UVec2::new(200, 60));

    let mut buffered = String::new();
    for frame in 0..3 {
        h.frame(|ui| {
            let committed = buffered_text_edit(ui, editor, id, &value, format_any, 80.0);
            assert!(
                committed.is_none(),
                "frame {frame}: an unfocused editor commits nothing"
            );
            buffered = ui.state_or_default::<EditBuffer>(id).text.clone();
        });
        assert_eq!(buffered, "42", "frame {frame}");
    }

    // The read-only fallback borrows the same buffer and refills it
    // unconditionally, so it carries the same hazard — and its overwrite
    // is what makes the field read-only in the first place.
    let read_only = WidgetId::from_hash("value_editor::read_only");
    for frame in 0..3 {
        h.frame(|ui| {
            assert!(
                read_only_label(ui, &theme.const_value_editor, read_only, &value).is_none(),
                "frame {frame}: a read-only field never commits"
            );
            buffered = ui.state_or_default::<EditBuffer>(read_only).text.clone();
        });
        assert_eq!(buffered, "42", "frame {frame}");
    }
}

/// Escape drops the draft: nothing commits, on the Escape frame or after, and the next idle
/// frame shows the document value again. Enter and a click elsewhere each commit what was
/// typed, once.
///
/// Commits are collected from every record pass, not only the input-observing one: a frame
/// can run a second pass, and the cancelled draft used to commit there.
#[test]
fn escape_drops_the_draft_and_enter_or_blur_commits() {
    #[derive(Debug)]
    struct Field<'a> {
        editor: &'a TextEditTheme,
        id: WidgetId,
        value: ConstValue,
    }
    /// Every commit any pass of a frame produced, and the text the field ends with.
    #[derive(Debug)]
    struct Framed {
        commits: Vec<String>,
        text: String,
    }
    impl Field<'_> {
        fn frame(&self, h: &mut UiHarness) -> Framed {
            let mut commits = Vec::new();
            let mut text = String::new();
            h.frame(|ui| {
                commits.extend(buffered_text_edit(
                    ui,
                    self.editor,
                    self.id,
                    &self.value,
                    format_any,
                    80.0,
                ));
                text.clone_from(&ui.state_or_default::<EditBuffer>(self.id).text);
            });
            Framed { commits, text }
        }

        fn focus_and_type(&self, h: &mut UiHarness) {
            let center = h.rect(self.id).expect("field arranged").center();
            h.click_at(center);
            assert!(self.frame(h).commits.is_empty(), "focusing commits nothing");
            h.key(Key::Char('7'));
            assert!(self.frame(h).commits.is_empty(), "typing commits nothing");
        }
    }

    let theme = Theme::default();
    let field = Field {
        editor: &theme.const_value_editor.drag_value.editor,
        id: WidgetId::from_hash("value_editor::commit_rules"),
        value: ConstValue::Int(42),
    };
    let mut h = UiHarness::new(UVec2::new(200, 60));
    field.frame(&mut h);

    field.focus_and_type(&mut h);
    h.key(Key::Escape);
    assert_eq!(
        field.frame(&mut h).commits,
        Vec::<String>::new(),
        "Escape must not commit"
    );
    for later in 0..3 {
        let Framed { commits, text } = field.frame(&mut h);
        assert!(
            commits.is_empty(),
            "frame {later} after Escape must not commit"
        );
        assert_eq!(
            text, "42",
            "frame {later}: the field shows the document value again"
        );
    }

    field.focus_and_type(&mut h);
    h.key(Key::Enter);
    assert_eq!(
        field.frame(&mut h).commits.len(),
        1,
        "Enter commits the draft once"
    );

    field.focus_and_type(&mut h);
    h.click_at(Vec2::new(190.0, 55.0));
    let commits: Vec<String> = (0..2).flat_map(|_| field.frame(&mut h).commits).collect();
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

/// [`format_any`] into a fresh buffer — the shape the assertions read.
fn formatted_any(value: &ConstValue) -> String {
    let mut out = String::new();
    format_any(value, &mut out);
    out
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
fn path_previews_distinguish_single_modes_and_multi_selections() {
    let single = |path, mode| PathPreview::single(path, mode).to_string();
    let multi = |paths: &[String]| PathPreview::multi(paths).to_string();

    assert_eq!(single("", FsPathMode::ExistingFile), "Choose file…");
    assert_eq!(single("", FsPathMode::NewFile), "Choose save path…");
    assert_eq!(single("", FsPathMode::Directory), "Choose directory…");
    assert_eq!(
        single("frames/light-01.raf", FsPathMode::ExistingFile),
        "light-01.raf"
    );
    // The name is a slice of the path it came from, not a rebuild of it —
    // which is what keeps a per-frame button label off the heap.
    assert!(matches!(
        PathPreview::single("frames/light-01.raf", FsPathMode::ExistingFile),
        PathPreview::Name(Cow::Borrowed("light-01.raf"))
    ));

    assert_eq!(multi(&[]), "Choose files…");
    assert_eq!(multi(&[String::new()]), "Choose files…");
    assert_eq!(multi(&["frames/light-01.raf".to_string()]), "1 file");
    assert_eq!(
        multi(&["a.raf".to_string(), "b.raf".to_string()]),
        "2 files"
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
}
