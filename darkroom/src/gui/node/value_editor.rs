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
//! dropdown over the port's declared variants. `Null` renders as a
//! read-only label.
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
    id: WidgetId,
    value: &StaticValue,
    data_type: &DataType,
) -> Option<StaticValue> {
    let width = theme.width;
    match value {
        StaticValue::Int(current) => {
            let buf = buffered_text_edit(ui, id, *current, i64::to_string, width);
            buf.parse::<i64>()
                .ok()
                .filter(|v| v != current)
                .map(StaticValue::Int)
        }
        StaticValue::Float(current) => {
            let buf = buffered_text_edit(ui, id, *current, format_float, width);
            buf.parse::<f64>()
                .ok()
                // Bit-exact: matches StaticValue's PartialEq, so we
                // don't emit a change for `1.0` → `1` (same value,
                // different textual form).
                .filter(|v| v.to_bits() != current.to_bits())
                .map(StaticValue::Float)
        }
        StaticValue::String(current) => {
            let buf = buffered_text_edit(ui, id, current.clone(), |s| s.clone(), width);
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
            // `MenuCommand::PickInputPath`; `App` opens the dialog outside
            // the record and applies the resulting path. So no synchronous
            // value here.
            Button::new()
                .id(id)
                .label(path_preview(path))
                .style(theme.button.clone())
                .text_wrap(TextWrap::Ellipsis)
                .size((Sizing::Fixed(width), Sizing::Hug))
                // Override the theme's vertical padding so the chip sits
                // flush on the port-row baseline.
                .padding(Spacing::xy(6.0, 0.0))
                .show(ui);
            None
        }
        StaticValue::Enum(current) => {
            // A dropdown over the port's declared variants. The variant
            // list + type identity live on `DataType::Enum`, not on the
            // value — without that we can't populate the menu, so fall
            // back to a read-only label (shouldn't happen: an `Enum`
            // value always rides an `Enum`-typed port).
            let DataType::Enum(def) = data_type else {
                return read_only_label(ui, id, value, width);
            };
            let options: Vec<&str> = def.variants.iter().map(String::as_str).collect();
            let before = options.iter().position(|v| *v == current).unwrap_or(0);
            let mut idx = before;
            // Hug + min width, not Fixed: the combo's label isn't ellipsized,
            // so a long variant (e.g. `sigma_clipped`) would overflow a fixed
            // field; let it grow to fit while keeping the field at least
            // `width` so short variants match the other editors.
            ComboBox::new(&mut idx, &options)
                .id(id)
                .style(theme.button.clone())
                .size((Sizing::Hug, Sizing::Hug))
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

/// Non-editable values (`Null`, or an `Enum` on a port that lost its
/// type) show their textual form in a read-only field; clicks fall
/// through to the surrounding row. Always returns `None`.
fn read_only_label(
    ui: &mut Ui,
    id: WidgetId,
    value: &StaticValue,
    width: f32,
) -> Option<StaticValue> {
    let mut buf = placeholder(value);
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
    canonical: T,
    fmt: fn(&T) -> String,
    width: f32,
) -> String {
    if ui.focused_id() != Some(id) {
        ui.state_mut::<EditBuffer>(id).text = fmt(&canonical);
    }
    let mut text = std::mem::take(&mut ui.state_mut::<EditBuffer>(id).text);
    TextEdit::new(&mut text)
        .id(id)
        .size((Sizing::Fixed(width), Sizing::Hug))
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

fn placeholder(value: &StaticValue) -> String {
    match value {
        StaticValue::Null => "null".to_owned(),
        StaticValue::Enum(variant) => variant.clone(),
        _ => String::new(),
    }
}
