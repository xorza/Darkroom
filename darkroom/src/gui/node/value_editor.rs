//! Inline editor for `Binding::Const(StaticValue)` on an input port.
//!
//! Renders next to the port label in the node body and returns a new
//! `StaticValue` when the user changes it. The host (`node_ui`)
//! converts that into an `Intent::SetInput { to: Binding::Const(..) }`.
//!
//! Supports `Int`, `Float`, `Bool`, `String`, and `FsPath` (a pick
//! button showing the chosen file's name — the actual path change comes
//! back deferred via the OS file dialog, opened by `App` outside the
//! record and polled here by `emit_path_picks`). `Null` and `Enum`
//! render as a read-only label.
//!
//! Textual edit state: a `TextEdit` round-trip through `i64`/`f64`
//! formatting would clobber partial input (typing "3." would reformat
//! to "3" on the next frame). The buffer lives in palantir's StateMap
//! keyed by the editor id; we mirror canonical → buffer only while
//! unfocused and parse on every frame, emitting a change only when the
//! parsed value differs from the canonical one.

use palantir::{Button, Checkbox, Configure, Sizing, TextEdit, TextWrap, Ui, WidgetId};
use scenarium::data::StaticValue;

#[derive(Default, Clone, Debug)]
struct EditBuffer {
    text: String,
}

/// Render the editor for `value`. Returns the new value when the user
/// edited it this frame, otherwise `None`. `id` must be stable across
/// frames so the TextEdit / buffer state survives. `width` is the
/// fixed logical-px width for the embedded TextEdit, threaded in
/// from the darkroom theme by the caller.
pub(crate) fn show(
    ui: &mut Ui,
    id: WidgetId,
    value: &StaticValue,
    width: f32,
) -> Option<StaticValue> {
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
        StaticValue::FsPath { path, .. } => {
            // A pick button showing the chosen file's name. The click is
            // polled by `emit_path_picks` (by `id`), which surfaces a
            // `MenuCommand::PickInputPath`; `App` opens the dialog outside
            // the record and applies the resulting path. So no synchronous
            // value here.
            Button::new()
                .id(id)
                .label(path_preview(path))
                .text_wrap(TextWrap::Ellipsis)
                .size((Sizing::Fixed(width), Sizing::Hug))
                .show(ui);
            None
        }
        StaticValue::Null | StaticValue::Enum { .. } => {
            // Not editable. Show the textual form so the value is
            // visible; clicks fall through to the surrounding row.
            let mut buf = placeholder(value);
            TextEdit::new(&mut buf)
                .id(id)
                .size((Sizing::Fixed(width), Sizing::Hug))
                .show(ui);
            None
        }
    }
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
        StaticValue::Enum { variant_name, .. } => variant_name.clone(),
        _ => String::new(),
    }
}
