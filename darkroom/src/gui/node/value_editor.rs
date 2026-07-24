//! Inline editor for `Binding::Const(StaticValue)` on an input port.
//!
//! Renders next to the port label in the node body and returns a new
//! `StaticValue` when the user changes it. The host (`node_ui`)
//! converts that into an `Intent::SetInput` carrying a constant binding.
//!
//! Supports `Int`, `Float`, `Bool`, `String`, and `FsPath` (a pick
//! button summarizing the chosen path or paths — `emit_path_picks` polls the click,
//! then `App` opens the OS file dialog after authoring). `Enum` renders as a
//! dropdown over the port's declared variants. `Any` renders as a
//! smart text field that infers the literal's kind from the text (see
//! [`parse_any`]).
//!
//! Numeric fields (`Int`/`Float`) render as an editable `DragValue`
//! ([`numeric_edit`]): drag horizontally to scrub, click to type an exact
//! value. `Any` stays a plain smart text field — its editing reinterprets
//! the literal's kind, which the numeric-only `DragValue` can't do.
//!
//! Every editor emits **once per committed gesture**, not per frame: the
//! `DragValue` reports its commit (drag release / Enter / blur), the text
//! editors commit on Enter or focus loss. Mid-gesture the document keeps
//! its old value — the widgets display their in-progress state themselves —
//! so one scrub or one typed entry lands as one `SetInput` undo step.
//!
//! Textual edit state: a `TextEdit` round-trip through `i64`/`f64`
//! formatting would clobber partial input (typing "3." would reformat
//! to "3" on the next frame). The buffer lives in aperture's StateMap
//! keyed by the editor id ([`crate::gui::widgets::buffered_edit::EditBuffer`],
//! shared with [`crate::gui::widgets::inline_rename`]'s renaming editor);
//! we mirror canonical → buffer only while unfocused — skipping the blur
//! frame, whose buffer still holds the user's text to commit — and parse
//! only when the edit commits.

use std::path::Path;

use aperture::{
    Button, Checkbox, ComboBox, Configure, DragValue, Sizing, TextEdit, TextEditTheme, TextWrap,
    Ui, WidgetId,
};
use scenarium::{DataType, FsPathMode, Library, StaticValue, ValueVariant};

use crate::gui::theme::StaticValueEditorTheme;
use crate::gui::widgets::buffered_edit::EditBuffer;

/// Render the editor for `value`. Returns the new value when the user
/// committed an edit this frame (scrub released, Enter, blur, or a
/// discrete pick), otherwise `None`. `id` must be stable across frames
/// so the TextEdit / buffer state survives. Every visual axis (button
/// look, field width) comes off `theme`.
pub(crate) fn show(
    ui: &mut Ui,
    theme: &StaticValueEditorTheme,
    library: &Library,
    id: WidgetId,
    value: &StaticValue,
    data_type: &DataType,
    value_variants: &[ValueVariant],
) -> Option<StaticValue> {
    let width = theme.width;
    // Picker variants (the input's `value_variants`, e.g. named config presets)
    // override the per-type editor: a dropdown of variant names, binding the
    // chosen variant's value. Works regardless of `data_type` (a custom config
    // port still shows its presets).
    if !value_variants.is_empty() {
        let names: Vec<&str> = value_variants
            .iter()
            .map(|o| o.display_name.as_str())
            .collect();
        let before = value_variants
            .iter()
            .position(|o| &o.value == value)
            .unwrap_or(0);
        let mut idx = before;
        ComboBox::new(&mut idx, &names)
            .id(id)
            .style(&theme.drag_value.chip)
            .size((Sizing::FILL, Sizing::FILL))
            .min_size((width, 0.0))
            .show(ui);
        return (idx != before)
            .then(|| value_variants.get(idx).map(|o| o.value.clone()))
            .flatten();
    }
    // The widget follows the *declared* port type, not the stored literal's
    // kind: a coerced or library-drifted literal still gets the declared
    // type's editor (displaying its coerced reading), and the next commit
    // stores the declared kind — re-canonicalizing the document. A literal
    // outside the type's coercion class falls back to a read-only label.
    let editor = &theme.drag_value.editor;
    match data_type {
        // An untyped (`Any`) port declares no concrete kind, so the literal's
        // kind is inferred from the text (see `parse_any`). Keyed on the port
        // type, not the stored value, so the field keeps reinterpreting across
        // kinds — typing "42" then "hello" flips `Int` → `String` — instead of
        // locking to the kind first entered.
        DataType::Any => any_smart_edit(ui, editor, id, value, width),
        DataType::Int => match value.as_i64() {
            Some(current) => int_edit(ui, theme, id, current, width),
            None => read_only_label(ui, editor, id, value, width),
        },
        DataType::Float => match value.as_f64() {
            Some(current) => float_edit(ui, theme, id, current, width),
            None => read_only_label(ui, editor, id, value, width),
        },
        DataType::Bool => {
            let Some(current) = value.as_bool() else {
                return read_only_label(ui, editor, id, value, width);
            };
            let mut draft = current;
            Checkbox::new(&mut draft).id(id).show(ui);
            (draft != current).then_some(StaticValue::Bool(draft))
        }
        DataType::String => {
            let Some(current) = value.as_string() else {
                return read_only_label(ui, editor, id, value, width);
            };
            let edit = buffered_text_edit(ui, editor, id, &current, |s| (*s).to_owned(), width);
            (edit.committed && edit.text != current).then_some(StaticValue::String(edit.text))
        }
        DataType::FsPath(config) => {
            // Preview whichever path literal is stored — a mode/kind mismatch
            // left by library drift still previews, and the pick dialog
            // (opened per the declared config) replaces it wholesale.
            let label = match value {
                StaticValue::FsPath(path) => single_path_preview(path, config.mode),
                StaticValue::FsPaths(paths) => multi_path_preview(paths),
                _ => return read_only_label(ui, editor, id, value, width),
            };
            // The blocking dialog runs after authoring, so this button only records its click.
            Button::new()
                .id(id)
                .label(label)
                .style(&theme.drag_value.chip)
                .text_wrap(TextWrap::Ellipsis)
                .size((Sizing::FILL, Sizing::FILL))
                .min_size((width, 0.0))
                .show(ui);
            None
        }
        DataType::Enum(type_id) => {
            // A dropdown over the port's registered variants. The variant list
            // lives on the library's `Enum` type entry, not on the value or the
            // id-only `DataType` — without it (an unregistered type) we can't
            // populate the menu, so fall back to a read-only label. A drifted
            // non-`Enum` literal seeds the first variant; any pick repairs it.
            let Some(variants) = library.enum_variants(type_id) else {
                return read_only_label(ui, editor, id, value, width);
            };
            let current = value.as_enum().unwrap_or_default();
            let options: Vec<&str> = variants.iter().map(String::as_str).collect();
            let before = options.iter().position(|v| *v == current).unwrap_or(0);
            let mut idx = before;
            ComboBox::new(&mut idx, &options)
                .id(id)
                .style(&theme.drag_value.chip)
                .size((Sizing::FILL, Sizing::FILL))
                .min_size((width, 0.0))
                .show(ui);
            if idx != before {
                options.get(idx).map(|v| StaticValue::Enum((*v).to_owned()))
            } else {
                None
            }
        }
        // No literal form (pick-or-wire ports carry variants, handled above).
        DataType::Custom(_) => read_only_label(ui, editor, id, value, width),
    }
}

/// Editor for an untyped (`Any`) port: one text field that reinterprets what
/// the user types into the tightest [`StaticValue`] — `true`/`false` → `Bool`,
/// an integer → `Int`, a finite decimal → `Float`, anything else → `String`.
/// The port declares no kind, so the kind rides on the value itself; the
/// ambiguity is inherent — `"42"` always reads back as `Int`, never the string
/// `"42"`. Returns the reinterpreted value only when the edit committed and
/// it differs from the current one.
fn any_smart_edit(
    ui: &mut Ui,
    editor: &TextEditTheme,
    id: WidgetId,
    value: &StaticValue,
    width: f32,
) -> Option<StaticValue> {
    let edit = buffered_text_edit(ui, editor, id, value, format_any, width);
    if !edit.committed {
        return None;
    }
    let parsed = parse_any(&edit.text);
    (parsed != *value).then_some(parsed)
}

/// Canonical text for an `Any` const, chosen so [`parse_any`] round-trips the
/// numeric and bool kinds: `Float` keeps its `.0` (else `3.0` would reparse as
/// `Int`) and `Null` shows blank (an unseeded `Any` starts empty). A `String`
/// prints verbatim, so a numeric-looking string (`"42"`) is the one kind that
/// doesn't round-trip — the accepted ambiguity of an untyped literal.
fn format_any(value: &StaticValue) -> String {
    match value {
        StaticValue::Null => String::new(),
        StaticValue::Float(v) => format_float(v),
        other => other.to_value_string(),
    }
}

/// Infer the tightest [`StaticValue`] from `text`: `true`/`false` (any casing)
/// → `Bool`, an `i64` → `Int`, a *finite* `f64` → `Float` (so `"nan"`/`"inf"`
/// stay text), else `String`. Order matters — bool before int before float, so
/// `"42"` is an `Int` and `"3.14"` a `Float`.
fn parse_any(text: &str) -> StaticValue {
    if text.eq_ignore_ascii_case("true") {
        return StaticValue::Bool(true);
    }
    if text.eq_ignore_ascii_case("false") {
        return StaticValue::Bool(false);
    }
    if let Ok(int) = text.parse::<i64>() {
        return StaticValue::Int(int);
    }
    if let Ok(float) = text.parse::<f64>()
        && float.is_finite()
    {
        return StaticValue::Float(float);
    }
    StaticValue::String(text.to_owned())
}

/// Non-editable fallback for an `Enum` on a port that lost its type (or a
/// stray `Null`): shows the textual form in a read-only field; clicks fall
/// through to the surrounding row. Always returns `None`.
fn read_only_label(
    ui: &mut Ui,
    editor: &TextEditTheme,
    id: WidgetId,
    value: &StaticValue,
    width: f32,
) -> Option<StaticValue> {
    let mut buf = value.to_value_string();
    TextEdit::new(&mut buf)
        .id(id)
        .style(editor)
        .size((Sizing::fixed(width), Sizing::FILL))
        .show(ui);
    None
}

fn single_path_preview(path: &str, mode: FsPathMode) -> String {
    let prompt = match mode {
        FsPathMode::ExistingFile => "Choose file…",
        FsPathMode::ExistingFiles => "Choose files…",
        FsPathMode::NewFile => "Choose save path…",
        FsPathMode::Directory => "Choose directory…",
    };
    if path.is_empty() {
        return prompt.to_owned();
    }
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

fn multi_path_preview(paths: &[String]) -> String {
    let count = paths.iter().filter(|path| !path.is_empty()).count();
    match count {
        0 => "Choose files…".to_owned(),
        1 => "1 file".to_owned(),
        _ => format!("{count} files"),
    }
}

/// What [`buffered_text_edit`] hands back: the buffer's current text and
/// whether this frame committed the edit (Enter, or focus left the field).
#[derive(Debug)]
struct TextEditOutcome {
    text: String,
    committed: bool,
}

/// Render a TextEdit whose buffer survives across frames via aperture's
/// StateMap. While the editor is unfocused, the buffer mirrors the
/// canonical value (re-formatted via `fmt`); while focused, the user's
/// in-progress text is left alone. The blur frame is detected *before*
/// the mirror (via [`EditBuffer::blur_edge`]) so the user's text
/// survives to be committed rather than being clobbered back to
/// canonical.
fn buffered_text_edit<T>(
    ui: &mut Ui,
    editor: &TextEditTheme,
    id: WidgetId,
    canonical: &T,
    fmt: fn(&T) -> String,
    width: f32,
) -> TextEditOutcome {
    let focused = ui.focused_id() == Some(id);
    let state = ui.state_mut::<EditBuffer>(id);
    let blurred = state.blur_edge(focused);
    if !focused && !blurred {
        state.text = fmt(canonical);
    }
    let mut text = std::mem::take(&mut ui.state_mut::<EditBuffer>(id).text);
    let submitted = TextEdit::new(&mut text)
        .id(id)
        .style(editor)
        .size((Sizing::fixed(width), Sizing::FILL))
        .show(ui)
        .submitted;
    let snapshot = text.clone();
    ui.state_mut::<EditBuffer>(id).text = text;
    TextEditOutcome {
        text: snapshot,
        committed: submitted || blurred,
    }
}

/// `{}` on f64 prints `1` for whole numbers, which round-trips through
/// `f64::parse` but reads as an integer to a user. `{:?}` keeps the
/// trailing `.0` so the field looks like a float.
fn format_float(v: &f64) -> String {
    format!("{v:?}")
}

/// Editor for an `Int` const: an editable `DragValue` — drag horizontally
/// to scrub, click to type an exact value. Both modes, the focus swap, and
/// Enter/blur commit live inside the widget. The draft re-seeds from the
/// document value every frame and is emitted only on the widget's
/// `committed` frame, which carries the gesture's final value — one scrub
/// or typed entry lands as one change.
fn int_edit(
    ui: &mut Ui,
    theme: &StaticValueEditorTheme,
    id: WidgetId,
    current: i64,
    width: f32,
) -> Option<StaticValue> {
    let mut draft = current;
    let committed = DragValue::new(&mut draft)
        .editable(true)
        .speed(int_speed(current))
        .style(&theme.drag_value)
        .size((Sizing::fixed(width), Sizing::FILL))
        .id(id)
        .show(ui)
        .committed;
    (committed && draft != current).then_some(StaticValue::Int(draft))
}

/// `Float` sibling of [`int_edit`].
fn float_edit(
    ui: &mut Ui,
    theme: &StaticValueEditorTheme,
    id: WidgetId,
    current: f64,
    width: f32,
) -> Option<StaticValue> {
    let mut draft = current;
    let committed = DragValue::new(&mut draft)
        .editable(true)
        .speed(float_speed(current))
        .decimals(3)
        .style(&theme.drag_value)
        .size((Sizing::fixed(width), Sizing::FILL))
        .id(id)
        .show(ui)
        .committed;
    // Bit-exact: matches StaticValue's PartialEq, so `1.0` → `1`
    // (same value, different textual form) doesn't emit a change.
    (committed && draft.to_bits() != current.to_bits()).then_some(StaticValue::Float(draft))
}

/// Drag speed for a float: ≈1% of the value's magnitude per logical pixel
/// (floored at a unit's worth), so scrubbing feels consistent whether the
/// value is `0.5` or `5000`. Sampled by `DragValue` at drag start, so it
/// stays fixed for the duration of a drag.
fn float_speed(v: f64) -> f64 {
    v.abs().max(1.0) * 0.01
}

/// Drag speed for an integer: same magnitude-relative scaling, floored at
/// `0.25`/px so small counts stay adjustable (~4 px per step near zero).
fn int_speed(v: i64) -> f64 {
    ((v.abs() as f64) * 0.01).max(0.25)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_any_infers_tightest_kind() {
        // Integers before floats: a bare integer is `Int`, not `Float`; the
        // sign is accepted by `i64::from_str`.
        assert_eq!(parse_any("42"), StaticValue::Int(42));
        assert_eq!(parse_any("-7"), StaticValue::Int(-7));
        assert_eq!(parse_any("+3"), StaticValue::Int(3));
        // Decimals / scientific / leading-or-trailing-dot fall through to float.
        assert_eq!(parse_any("2.5"), StaticValue::Float(2.5));
        assert_eq!(parse_any(".5"), StaticValue::Float(0.5));
        assert_eq!(parse_any("5."), StaticValue::Float(5.0));
        assert_eq!(parse_any("1e3"), StaticValue::Float(1000.0));
        // `true`/`false` in any casing are bools; a non-bool word stays text.
        assert_eq!(parse_any("true"), StaticValue::Bool(true));
        assert_eq!(parse_any("false"), StaticValue::Bool(false));
        assert_eq!(parse_any("True"), StaticValue::Bool(true));
        assert_eq!(parse_any("TRUE"), StaticValue::Bool(true));
        assert_eq!(parse_any("False"), StaticValue::Bool(false));
        assert_eq!(parse_any("yes"), StaticValue::String("yes".into()));
        // Non-finite floats parse as `f64` but are rejected — they stay text
        // rather than becoming `Float(inf)`/`Float(nan)`.
        assert_eq!(parse_any("inf"), StaticValue::String("inf".into()));
        assert_eq!(parse_any("nan"), StaticValue::String("nan".into()));
        // Empty, and numeric-with-suffix, are plain strings.
        assert_eq!(parse_any(""), StaticValue::String(String::new()));
        assert_eq!(parse_any("hello"), StaticValue::String("hello".into()));
        assert_eq!(parse_any("42x"), StaticValue::String("42x".into()));
    }

    #[test]
    fn format_any_round_trips_non_string_kinds() {
        // Int / Float / Bool survive a format→parse round-trip unchanged, so the
        // editor can reformat on blur without flipping the kind. `Float` keeps
        // its `.0` so a whole-number float doesn't collapse back to `Int`.
        for value in [
            StaticValue::Int(42),
            StaticValue::Float(3.0),
            StaticValue::Float(2.5),
            StaticValue::Float(1000.0),
            StaticValue::Bool(true),
            StaticValue::Bool(false),
        ] {
            assert_eq!(
                parse_any(&format_any(&value)),
                value,
                "round-trip {value:?}"
            );
        }
        assert_eq!(format_any(&StaticValue::Float(3.0)), "3.0");
        // `Null` (an unseeded `Any`) shows blank rather than the text "null".
        assert_eq!(format_any(&StaticValue::Null), "");
        // A numeric-looking string is the one kind that doesn't round-trip — it
        // reparses as the number. The accepted ambiguity of an untyped literal.
        assert_eq!(
            parse_any(&format_any(&StaticValue::String("42".into()))),
            StaticValue::Int(42)
        );
    }

    #[test]
    fn path_previews_distinguish_single_modes_and_multi_selections() {
        assert_eq!(
            single_path_preview("", FsPathMode::ExistingFile),
            "Choose file…"
        );
        assert_eq!(
            single_path_preview("", FsPathMode::NewFile),
            "Choose save path…"
        );
        assert_eq!(
            single_path_preview("", FsPathMode::Directory),
            "Choose directory…"
        );
        assert_eq!(
            single_path_preview("frames/light-01.raf", FsPathMode::ExistingFile),
            "light-01.raf"
        );
        assert_eq!(multi_path_preview(&[]), "Choose files…");
        assert_eq!(multi_path_preview(&[String::new()]), "Choose files…");
        assert_eq!(
            multi_path_preview(&["frames/light-01.raf".to_string()]),
            "1 file"
        );
        assert_eq!(
            multi_path_preview(&["a.raf".to_string(), "b.raf".to_string()]),
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
}
