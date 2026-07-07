//! Inline editor for `Binding::Const(StaticValue)` on an input port.
//!
//! Renders next to the port label in the node body and returns a new
//! `StaticValue` when the user changes it. The host (`node_ui`)
//! converts that into an `Intent::SetInput { to: Binding::Const(..) }`.
//!
//! Supports `Int`, `Float`, `Bool`, `String`, and `FsPath` (a pick
//! button showing the chosen file's name — the actual path change comes
//! back deferred via the OS file dialog, opened by `App` outside the
//! record and polled here by `emit_path_picks`). `Enum` renders as a
//! dropdown over the port's declared variants. `Any` renders as a
//! smart text field that infers the literal's kind from the text (see
//! [`parse_any`]).
//!
//! Textual edit state: a `TextEdit` round-trip through `i64`/`f64`
//! formatting would clobber partial input (typing "3." would reformat
//! to "3" on the next frame). The buffer lives in palantir's StateMap
//! keyed by the editor id; we mirror canonical → buffer only while
//! unfocused and parse on every frame, emitting a change only when the
//! parsed value differs from the canonical one.

use palantir::{
    Button, Checkbox, ComboBox, Configure, Sizing, Spacing, TextEdit, TextWrap, Ui, WidgetId,
};
use scenarium::data::{DataType, StaticValue};
use scenarium::library::Library;
use scenarium::node::function::ValueVariant;

use crate::gui::theme::StaticValueEditorTheme;

#[derive(Default, Clone, Debug)]
struct EditBuffer {
    text: String,
}

/// Render the editor for `value`. Returns the new value when the user
/// edited it this frame, otherwise `None`. `id` must be stable across
/// frames so the TextEdit / buffer state survives. Every visual axis
/// (button look, field width) comes off `theme`.
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
            .style(theme.button.clone())
            .size((Sizing::FILL, Sizing::Hug))
            .min_size((width, 0.0))
            .show(ui);
        return (idx != before)
            .then(|| value_variants.get(idx).map(|o| o.value.clone()))
            .flatten();
    }
    // An untyped (`Any`) port declares no concrete kind, so the literal's kind
    // is inferred from the text (see `parse_any`). Keyed on the port type, not
    // the stored value, so the field keeps reinterpreting across kinds — typing
    // "42" then "hello" flips `Int` → `String` — instead of locking to the kind
    // first entered.
    if matches!(data_type, DataType::Any) {
        return any_smart_edit(ui, id, value, width);
    }
    match value {
        StaticValue::Int(current) => {
            let buf = buffered_text_edit(ui, id, current, i64::to_string, width);
            buf.parse::<i64>()
                .ok()
                .filter(|v| v != current)
                .map(StaticValue::Int)
        }
        StaticValue::Float(current) => {
            let buf = buffered_text_edit(ui, id, current, format_float, width);
            buf.parse::<f64>()
                .ok()
                // Bit-exact: matches StaticValue's PartialEq, so we
                // don't emit a change for `1.0` → `1` (same value,
                // different textual form).
                .filter(|v| v.to_bits() != current.to_bits())
                .map(StaticValue::Float)
        }
        StaticValue::String(current) => {
            let buf = buffered_text_edit(ui, id, current, |s| s.clone(), width);
            (buf != *current).then_some(StaticValue::String(buf))
        }
        StaticValue::Bool(current) => {
            let mut draft = *current;
            Checkbox::new(&mut draft).id(id).show(ui);
            (draft != *current).then_some(StaticValue::Bool(draft))
        }
        StaticValue::FsPath(path) => {
            // A pick button showing the chosen file's name. The click is
            // polled by `emit_path_picks` (by `id`), which surfaces a
            // `AppCommand::PickInputPath`; `App` opens the dialog outside
            // the record and applies the resulting path. So no synchronous
            // value here.
            Button::new()
                .id(id)
                .label(path_preview(path))
                .style(theme.button.clone())
                .text_wrap(TextWrap::Ellipsis)
                .size((Sizing::FILL, Sizing::Hug))
                .min_size((width, 0.0))
                // Override the theme's vertical padding so the chip sits
                // flush on the port-row baseline.
                .padding(Spacing::xy(6.0, 0.0))
                .show(ui);
            None
        }
        StaticValue::Enum(current) => {
            // A dropdown over the port's registered variants. The variant
            // list lives on the library's `Enum` type entry, not on the value
            // or the id-only `DataType` — without it (a non-`Enum` port, or an
            // unregistered type) we can't populate the menu, so fall back to a
            // read-only label (shouldn't happen: an `Enum` value always rides a
            // registered `Enum`-typed port).
            let DataType::Enum(type_id) = data_type else {
                return read_only_label(ui, id, value, width);
            };
            let Some(variants) = library.enum_variants(type_id) else {
                return read_only_label(ui, id, value, width);
            };
            let options: Vec<&str> = variants.iter().map(String::as_str).collect();
            let before = options.iter().position(|v| *v == current).unwrap_or(0);
            let mut idx = before;
            ComboBox::new(&mut idx, &options)
                .id(id)
                .style(theme.button.clone())
                .size((Sizing::FILL, Sizing::Hug))
                .min_size((width, 0.0))
                .show(ui);
            if idx != before {
                options.get(idx).map(|v| StaticValue::Enum((*v).to_owned()))
            } else {
                None
            }
        }
        StaticValue::Null => read_only_label(ui, id, value, width),
    }
}

/// Editor for an untyped (`Any`) port: one text field that reinterprets what
/// the user types into the tightest [`StaticValue`] — `true`/`false` → `Bool`,
/// an integer → `Int`, a finite decimal → `Float`, anything else → `String`.
/// The port declares no kind, so the kind rides on the value itself; the
/// ambiguity is inherent — `"42"` always reads back as `Int`, never the string
/// `"42"`. Returns the reinterpreted value only when it differs from the
/// current one.
fn any_smart_edit(
    ui: &mut Ui,
    id: WidgetId,
    value: &StaticValue,
    width: f32,
) -> Option<StaticValue> {
    let buf = buffered_text_edit(ui, id, value, format_any, width);
    let parsed = parse_any(&buf);
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
    id: WidgetId,
    value: &StaticValue,
    width: f32,
) -> Option<StaticValue> {
    let mut buf = value.to_value_string();
    TextEdit::new(&mut buf)
        .id(id)
        .size((Sizing::Fixed(width), Sizing::Hug))
        .show(ui);
    None
}

/// The pick button's label: the chosen file's name (last path
/// component), or a prompt when no path is set yet.
fn path_preview(path: &str) -> String {
    if path.is_empty() {
        return "Choose file…".to_owned();
    }
    std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

/// Render a TextEdit whose buffer survives across frames via palantir's
/// StateMap. While the editor is unfocused, the buffer mirrors the
/// canonical value (re-formatted via `fmt`); while focused, the user's
/// in-progress text is left alone. Returns the current buffer for the
/// caller to parse.
fn buffered_text_edit<T>(
    ui: &mut Ui,
    id: WidgetId,
    canonical: &T,
    fmt: fn(&T) -> String,
    width: f32,
) -> String {
    if ui.focused_id() != Some(id) {
        ui.state_mut::<EditBuffer>(id).text = fmt(canonical);
    }
    let mut text = std::mem::take(&mut ui.state_mut::<EditBuffer>(id).text);
    TextEdit::new(&mut text)
        .id(id)
        .size((Sizing::FILL, Sizing::Hug))
        .min_size((width, 0.0))
        .show(ui);
    let snapshot = text.clone();
    ui.state_mut::<EditBuffer>(id).text = text;
    snapshot
}

/// `{}` on f64 prints `1` for whole numbers, which round-trips through
/// `f64::parse` but reads as an integer to a user. `{:?}` keeps the
/// trailing `.0` so the field looks like a float.
fn format_float(v: &f64) -> String {
    format!("{v:?}")
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
}
