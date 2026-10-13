//! Inline editor for `Binding::Const(ConstValue)` on an input port.
//!
//! Renders next to the port label in the node body and returns a new
//! `ConstValue` when the user changes it. The host (`node_ui`)
//! converts that into an `GraphIntent::SetInput` carrying a constant binding.
//!
//! Supports `Int`, `Float`, `Bool`, `String`, and `FsPath` (a pick button
//! summarizing the chosen path or paths — the port row's value cell polls the
//! click, then `App` opens the OS file dialog after authoring). `Enum` renders
//! as a dropdown over the port's declared variants. `Any` renders as a smart
//! text field that infers the literal's kind from the text (see
//! [`parse_any`]).
//!
//! Numeric fields (`Int`/`Float`) render as an editable `DragValue`
//! (a `DragValue` field): drag horizontally to scrub, click to type an exact
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
//! to "3" on the next frame). The buffer lives in palantir's `StateMap`
//! keyed by the editor id ([`crate::gui::widgets::edit_buffer::EditBuffer`]);
//! we mirror canonical → buffer only while unfocused — skipping the blur
//! frame, whose buffer still holds the user's text to commit — and parse
//! only when the edit commits.

use std::borrow::Cow;
use std::fmt;
use std::fmt::Formatter;
use std::fmt::{Display, Write as _};
use std::path::Path;

use palantir::prelude::*;
use palantir::{TextEditTheme, TextWrap};
use scenarium::{ConstValue, DataType, FsPathMode, Library, ValueVariant};

use crate::gui::theme::const_value_editor_theme::ConstValueEditorTheme;
use crate::gui::widgets::edit_buffer::EditBuffer;

/// Render the editor for `value`. Returns the new value when the user
/// committed an edit this frame (scrub released, Enter, blur, or a
/// discrete pick), otherwise `None`. `id` must be stable across frames
/// so the `TextEdit` / buffer state survives. Every visual axis (button
/// look, field width) comes off `theme`.
pub(super) fn show(
    ui: &mut Ui,
    theme: &ConstValueEditorTheme,
    library: &Library,
    id: WidgetId,
    value: &ConstValue,
    data_type: &DataType,
    value_variants: &[ValueVariant],
) -> Option<ConstValue> {
    let width = theme.width;
    // Picker variants (the input's `value_variants`, e.g. named config presets)
    // override the per-type editor: a dropdown of variant names, binding the
    // chosen variant's value. Works regardless of `data_type` (a custom config
    // port still shows its presets).
    if !value_variants.is_empty() {
        let before = value_variants
            .iter()
            .position(|o| &o.value == value)
            .unwrap_or(0);
        let picked = combo_pick(ui, theme, id, value_variants, ValueVariant::label, before)?;
        return value_variants.get(picked).map(|o| o.value.clone());
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
        DataType::Int => {
            let Some(current) = value.as_i64() else {
                return read_only_label(ui, theme, id, value);
            };
            int_edit(ui, theme, id, current)
        }
        DataType::Float => {
            let Some(current) = value.as_f64() else {
                return read_only_label(ui, theme, id, value);
            };
            float_edit(ui, theme, id, current)
        }
        DataType::Bool => {
            let Some(current) = value.as_bool() else {
                return read_only_label(ui, theme, id, value);
            };
            let mut draft = current;
            Checkbox::new(&mut draft).id(id).show(ui);
            (draft != current).then_some(ConstValue::Bool(draft))
        }
        DataType::String => {
            let Some(current) = value.as_string() else {
                return read_only_label(ui, theme, id, value);
            };
            let edited = buffered_text_edit(ui, editor, id, current, format_string, width)?;
            (edited != current).then_some(ConstValue::String(edited))
        }
        DataType::FsPath(config) => {
            // Preview whichever path literal is stored — a mode/kind mismatch
            // left by library drift still previews, and the pick dialog
            // (opened per the declared config) replaces it wholesale.
            let preview = match value {
                ConstValue::FsPath(path) => PathPreview::single(path, config.mode),
                ConstValue::FsPaths(paths) => PathPreview::multi(paths),
                _ => return read_only_label(ui, theme, id, value),
            };
            let label = fmt!(ui, "{preview}");
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
            // id-only `DataType` — without it (an unregistered type, or one
            // registered with no variants) we can't populate the menu, so fall
            // back to a read-only label. A drifted non-`Enum` literal seeds the
            // first variant; any pick repairs it.
            let Some(variants) = library.enum_variants(*type_id).filter(|v| !v.is_empty()) else {
                return read_only_label(ui, theme, id, value);
            };
            let current = value.as_enum().unwrap_or_default();
            let before = variants.iter().position(|v| v == current).unwrap_or(0);
            let picked = combo_pick(ui, theme, id, variants, String::as_str, before)?;
            variants.get(picked).cloned().map(ConstValue::Enum)
        }
        // No literal form (pick-or-wire ports carry variants, handled above).
        DataType::Custom(_) => read_only_label(ui, theme, id, value),
    }
}

/// A dropdown over `options`, returning the newly picked index — `None` when
/// the selection is unchanged. Both pickers (the value-variant override and
/// the `Enum` port) are this widget over different option lists, each read
/// through its own `label`.
///
/// Generic over the row so each caller hands its own collection over — a
/// port's `&[ValueVariant]`, an `Enum` type's `&[String]` — rather than
/// projecting the labels into a fresh `Vec<&str>` on every frame the port
/// records.
fn combo_pick<S>(
    ui: &mut Ui,
    theme: &ConstValueEditorTheme,
    id: WidgetId,
    options: &[S],
    label: fn(&S) -> &str,
    before: usize,
) -> Option<usize> {
    let mut idx = before;
    ComboBox::labeled(&mut idx, options, label)
        .id(id)
        .button_style(&theme.drag_value.chip)
        .size((Sizing::FILL, Sizing::FILL))
        .min_size((theme.width, 0.0))
        .show(ui);
    (idx != before).then_some(idx)
}

/// Editor for an untyped (`Any`) port: one text field that reinterprets what
/// the user types into the tightest [`ConstValue`] — `true`/`false` → `Bool`,
/// an integer → `Int`, a finite decimal → `Float`, anything else → `String`.
/// The port declares no kind, so the kind rides on the value itself; the
/// ambiguity is inherent — `"42"` always reads back as `Int`, never the string
/// `"42"`. Returns the reinterpreted value only when the edit committed and
/// it differs from the current one.
fn any_smart_edit(
    ui: &mut Ui,
    editor: &TextEditTheme,
    id: WidgetId,
    value: &ConstValue,
    width: f32,
) -> Option<ConstValue> {
    let text = buffered_text_edit(ui, editor, id, value, format_any, width)?;
    let parsed = parse_any(&text);
    (parsed != *value).then_some(parsed)
}

/// Canonical text for an `Any` const, chosen so [`parse_any`] round-trips the
/// numeric and bool kinds: `Float` keeps its `.0` (else `3.0` would reparse as
/// `Int`) and `Null` shows blank (an unseeded `Any` starts empty). A `String`
/// prints verbatim, so a numeric-looking string (`"42"`) is the one kind that
/// doesn't round-trip — the accepted ambiguity of an untyped literal.
///
/// Writes into the caller's cleared buffer — see [`buffered_text_edit`].
fn format_any(value: &ConstValue, out: &mut String) {
    match value {
        ConstValue::Null => {}
        ConstValue::Float(v) => format_float(*v, out),
        other => {
            write!(out, "{}", other.value_text()).expect("writing to a String cannot fail");
        }
    }
}

/// Infer the tightest [`ConstValue`] from `text`: `true`/`false` (any casing)
/// → `Bool`, an `i64` → `Int`, a *finite* `f64` → `Float` (so `"nan"`/`"inf"`
/// stay text), else `String`. Order matters — bool before int before float, so
/// `"42"` is an `Int` and `"3.14"` a `Float`.
fn parse_any(text: &str) -> ConstValue {
    if text.eq_ignore_ascii_case("true") {
        return ConstValue::Bool(true);
    }
    if text.eq_ignore_ascii_case("false") {
        return ConstValue::Bool(false);
    }
    if let Ok(int) = text.parse::<i64>() {
        return ConstValue::Int(int);
    }
    if let Ok(float) = text.parse::<f64>()
        && float.is_finite()
    {
        return ConstValue::Float(float);
    }
    ConstValue::String(text.to_owned())
}

/// Non-editable fallback for a literal outside its port's coercion class —
/// a drifted kind, a `Custom` port's stray const, an unregistered enum, a
/// `Null`: shows the textual form in a read-only field; clicks fall
/// through to the surrounding row. Always returns `None`.
fn read_only_label(
    ui: &mut Ui,
    theme: &ConstValueEditorTheme,
    id: WidgetId,
    value: &ConstValue,
) -> Option<ConstValue> {
    // The same retained buffer the editable fields keep under this id,
    // refilled from the literal every frame — which is also what makes the
    // field read-only: anything typed into it is gone by the next record.
    ui.with_state::<EditBuffer, _>(id, |ui, buffer| {
        let text = &mut buffer.text;
        text.clear();
        write!(text, "{}", value.value_text()).expect("writing to a String cannot fail");
        TextEdit::new(text)
            .id(id)
            .style(&theme.drag_value.editor)
            .size((Sizing::fixed(theme.width), Sizing::FILL))
            .show(ui);
    });
    None
}

/// What an `FsPath` input's pick button reads: the mode's call to action
/// while nothing is chosen, a single chosen path's own file name, or how many
/// files a multi-pick holds.
///
/// Rendered on demand rather than assembled into a `String`: the button
/// records every frame the node does, and the label goes straight into the
/// record pass's text arena.
#[derive(Debug)]
enum PathPreview<'a> {
    /// Nothing chosen — the prompt for the mode being picked for.
    Prompt(&'static str),
    /// The chosen path's final component (the whole path when it has none).
    Name(Cow<'a, str>),
    /// How many non-empty paths a multi-pick holds. Never zero — that reads
    /// as a [`Prompt`](Self::Prompt).
    Count(usize),
}

impl<'a> PathPreview<'a> {
    /// The label for a single-path literal.
    fn single(path: &'a str, mode: FsPathMode) -> Self {
        if path.is_empty() {
            return Self::Prompt(match mode {
                FsPathMode::ExistingFile => "Choose file…",
                FsPathMode::ExistingFiles => "Choose files…",
                FsPathMode::NewFile => "Choose save path…",
                FsPathMode::Directory => "Choose directory…",
            });
        }
        Self::Name(
            Path::new(path)
                .file_name()
                .map_or(Cow::Borrowed(path), |name| name.to_string_lossy()),
        )
    }

    /// The label for a multi-path literal: a count, since no one file name
    /// stands for the selection.
    fn multi(paths: &[String]) -> Self {
        match paths.iter().filter(|path| !path.is_empty()).count() {
            0 => Self::Prompt("Choose files…"),
            count => Self::Count(count),
        }
    }
}

impl Display for PathPreview<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Prompt(prompt) => f.write_str(prompt),
            Self::Name(name) => f.write_str(name),
            Self::Count(1) => f.write_str("1 file"),
            Self::Count(count) => write!(f, "{count} files"),
        }
    }
}

/// Render a `TextEdit` whose buffer survives across frames via palantir's
/// `StateMap`. Returns the buffer's text on the frame the edit commits (Enter,
/// or focus left the field), `None` on every other — Escape included, which
/// drops the draft and lets the next idle frame refill it.
///
/// While the field is idle, `mirror` refills the cleared buffer from the
/// canonical value rather than handing back a fresh `String`, so the frames
/// that change nothing cost no allocation. While focused, and on the frame
/// focus leaves, the user's text is left alone so it survives to be committed
/// ([`EditBuffer::is_idle`]).
fn buffered_text_edit<T: ?Sized>(
    ui: &mut Ui,
    editor: &TextEditTheme,
    id: WidgetId,
    canonical: &T,
    mirror: fn(&T, &mut String),
    width: f32,
) -> Option<String> {
    ui.with_state::<EditBuffer, _>(id, |ui, buffer| {
        let idle = buffer.is_idle(ui.focus() == Some(id));
        let text = &mut buffer.text;
        if idle {
            text.clear();
            mirror(canonical, text);
        }
        let response = TextEdit::new(text)
            .id(id)
            .style(editor)
            .size((Sizing::fixed(width), Sizing::FILL))
            .show(ui);
        // The buffer keeps the text — the editor goes on showing it until
        // the mirror re-seeds from the committed document value — so a
        // commit is the one frame that copies, at gesture rate rather than
        // frame rate.
        let committed = response.committed.then(|| text.clone());
        buffer.settle(ui.focus() == Some(id));
        committed
    })
}

/// The `String` port's mirror: its literal *is* the field's text, verbatim.
fn format_string(value: &str, out: &mut String) {
    out.push_str(value);
}

/// `{}` on f64 prints `1` for whole numbers, which round-trips through
/// `f64::parse` but reads as an integer to a user. `{:?}` keeps the
/// trailing `.0` so the field looks like a float.
fn format_float(v: f64, out: &mut String) {
    write!(out, "{v:?}").expect("writing to a String cannot fail");
}

/// Editor for an `Int` const: an editable `DragValue` — drag horizontally
/// to scrub, click to type an exact value. Both modes, the focus swap, and
/// Enter/blur commit live inside the widget. The draft re-seeds from the
/// document value every frame and is emitted only on the widget's
/// `committed` frame, which carries the gesture's final value — one scrub
/// or typed entry lands as one change.
fn int_edit(
    ui: &mut Ui,
    theme: &ConstValueEditorTheme,
    id: WidgetId,
    current: i64,
) -> Option<ConstValue> {
    let mut draft = current;
    let committed = DragValue::new(&mut draft)
        .editable(true)
        .speed(int_speed(current))
        .style(&theme.drag_value)
        .size((Sizing::fixed(theme.width), Sizing::FILL))
        .id(id)
        .show(ui)
        .committed;
    (committed && draft != current).then_some(ConstValue::Int(draft))
}

/// `Float` sibling of [`int_edit`].
fn float_edit(
    ui: &mut Ui,
    theme: &ConstValueEditorTheme,
    id: WidgetId,
    current: f64,
) -> Option<ConstValue> {
    let mut draft = current;
    let committed = DragValue::new(&mut draft)
        .editable(true)
        .speed(float_speed(current))
        .decimals(3)
        .style(&theme.drag_value)
        .size((Sizing::fixed(theme.width), Sizing::FILL))
        .id(id)
        .show(ui)
        .committed;
    // Bit-exact: matches ConstValue's PartialEq, so `1.0` → `1`
    // (same value, different textual form) doesn't emit a change.
    (committed && draft.to_bits() != current.to_bits()).then_some(ConstValue::Float(draft))
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
mod tests;
