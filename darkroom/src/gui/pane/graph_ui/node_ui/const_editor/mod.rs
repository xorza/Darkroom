//! Inline editor for `Binding::Const(ConstValue)` on an input port.
//!
//! Renders next to the port label in the node body and returns a new `ConstValue` when the
//! user changes it. The host (`node_ui`) converts that into a `GraphIntent::SetInput`
//! carrying a constant binding.
//!
//! Supports `Int`, `Float`, `Bool`, `String`, and a path (a pick button summarizing the
//! chosen path or paths; the port row's value cell polls the click, then `App` opens the OS
//! file dialog after authoring). `Enum` renders as a dropdown over the port's declared
//! variants. `Any` renders as a smart text field that infers the literal's kind from the
//! text (see [`parse_any`]).
//!
//! Numeric fields (`Int`/`Float`) render as an editable `DragValue`: drag horizontally to
//! scrub, click to type an exact value. `Any` stays a text field, as its editing
//! reinterprets the literal's kind, which `DragValue` can't do.
//!
//! Every editor emits **once per committed gesture**, not per frame: the `DragValue` reports
//! its commit (drag release, Enter or blur), the text editors commit on Enter or focus loss.
//! Mid-gesture the document keeps its old value, so one scrub or one typed entry lands as one
//! `SetInput` undo step.
//!
//! A `TextEdit` round-trip through `i64`/`f64` formatting would clobber partial input (typing
//! "3." would reformat to "3" on the next frame). The text fields are palantir's buffered
//! `TextEdit`s, which keep the draft under the editor id, take the document value only while
//! idle, and hand the draft over on its commit, which is when it is parsed.

use std::fmt::{self as std_fmt, Display, Formatter};

use palantir::{
    Align, Button, Checkbox, ComboBox, Configure, DragValue, Panel, SizeSpec, Text, TextEdit,
    TextInput, TextWrap, Ui, VAlign, WidgetId, fmt,
};
use scenarium::{ConstValue, DataType, FloatSpan, IntSpan, Library, ValueDomain, ValueVariant};

use crate::gui::path_type::PathType;
use crate::gui::theme::const_editor_theme::ConstEditorTheme;
use crate::gui::widgets::format::path_preview::PathPreview;
use crate::gui::widgets::format::shown_value::{self, FLOAT_DECIMALS, ShownValue, UNSET};
use crate::gui::widgets::support;

/// One const editor: the field it records as, and the literal it edits on
/// a port of `data_type`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ConstEditor<'a> {
    pub(crate) field: Field<'a>,
    pub(crate) library: &'a Library,
    pub(crate) value: &'a ConstValue,
    pub(crate) data_type: DataType,
    /// The constants the port admits: a picker overrides the per-type editor, and a span
    /// bounds a number's field and shows how far along it the value sits.
    pub(crate) domain: &'a ValueDomain,
}

/// Where one editor records: its stable id, and the theme every visual axis
/// (button look, field width) comes off.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Field<'a> {
    pub(crate) theme: &'a ConstEditorTheme,
    /// Stable across frames, so the `TextEdit` / buffer state survives.
    pub(crate) id: WidgetId,
}

/// A dropdown's options, each read through `label`, with `current` picked.
#[derive(Debug)]
struct DropdownOptions<'a, S> {
    options: &'a [S],
    label: fn(&S) -> &str,
    current: &'a S,
}

/// Hand-written: `derive` would demand `S: Copy`, and only a borrow of the
/// options is held.
impl<S> Clone for DropdownOptions<'_, S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for DropdownOptions<'_, S> {}

impl ConstEditor<'_> {
    /// Render the editor for the value. Returns the new value when the user
    /// committed an edit this frame (scrub released, Enter, blur, or a
    /// discrete pick), otherwise `None`.
    pub(crate) fn show(self, ui: &mut Ui) -> Option<ConstValue> {
        let ConstEditor {
            field,
            library,
            value,
            data_type,
            domain,
        } = self;
        // A picker (named config presets) overrides the per-type editor: a dropdown of variant
        // names, binding the chosen variant's value, whatever the `data_type`.
        let value_variants = domain.variants();
        if !value_variants.is_empty() {
            let current = value_variants
                .iter()
                .find(|o| &o.value == value)
                .expect("an admitted picker value is one of its variants");
            let picked = field.combo(
                ui,
                DropdownOptions {
                    options: value_variants,
                    label: |variant: &ValueVariant| variant.label.as_str(),
                    current,
                },
            )?;
            return Some(picked.value.clone());
        }
        // A type the library does not register, which a compound's interface keeps as written,
        // has no editor here: its constant shows as written and stays.
        if !library.knows(data_type) {
            field.read_only(ui, ShownValue { value, data_type });
            return None;
        }
        // The widget follows the declared port type, not the stored literal's kind: a coerced
        // literal gets the declared type's editor, and the next commit stores the declared kind.
        // An admitted literal is in the type's coercion class, or `Null` on an optional input
        // left unset, which shows as a read-only label.
        match data_type {
            // An untyped port declares no concrete kind, so the literal's kind is inferred from
            // the text (see `parse_any`). Keyed on the port type, not the stored value, so
            // typing "42" then "hello" flips `Int` → `String` rather than locking to the first
            // kind entered.
            DataType::Any => field.any_smart(ui, value),
            DataType::Int => {
                let Some(cur) = value.as_i64() else {
                    return field.unset(ui, value);
                };
                let span = match domain {
                    ValueDomain::Int(span) => Some(*span),
                    _ => None,
                };
                field.int(ui, cur, span)
            }
            DataType::Float => {
                let Some(cur) = value.as_f64() else {
                    return field.unset(ui, value);
                };
                let span = match domain {
                    ValueDomain::Float(span) => Some(*span),
                    _ => None,
                };
                field.float(ui, cur, span)
            }
            DataType::Bool => {
                let Some(cur) = value.as_bool() else {
                    return field.unset(ui, value);
                };
                let mut draft = cur;
                Checkbox::new(&mut draft).id(field.id).show(ui);
                (draft != cur).then_some(ConstValue::Bool(draft))
            }
            DataType::String => {
                let Some(cur) = value.as_string() else {
                    return field.unset(ui, value);
                };
                field.text(ui, cur).map(ConstValue::String)
            }
            DataType::Dir | DataType::File { .. } => {
                // The admitted literal is the shape the mode takes; the pick dialog replaces it
                // wholesale.
                let path_type = PathType::of(data_type).expect("a directory or a file is a path");
                let preview = match value {
                    ConstValue::FsPath(path) => PathPreview::single(path, path_type),
                    ConstValue::FsPaths(paths) => PathPreview::multi(paths),
                    _ => return field.unset(ui, value),
                };
                let label = fmt!(ui, "{preview}");
                // A file name cut in the middle keeps its extension, which tells one model or
                // frame from the next; the whole path is the tooltip.
                let tooltip = match (&preview, value) {
                    (PathPreview::Name(_), ConstValue::FsPath(path)) => Some(path.as_str()),
                    _ => None,
                };
                // The blocking dialog runs after authoring, so this button only records its click.
                let button = Button::new()
                    .id(field.id)
                    .label(label)
                    .style(&field.theme.button)
                    .text_wrap(TextWrap::EllipsisMiddle)
                    .text_align(Align::LEFT)
                    .size(SizeSpec::FILL)
                    .min_size((field.theme.width, 0.0))
                    .show(ui);
                support::tooltip_after(ui, &button, tooltip);
                None
            }
            DataType::Enum(enum_id) => {
                // The variant list lives on the library's enum, not on the value or the id-only
                // `DataType`; an admitted literal names one of them.
                let Some(cur) = value.as_enum() else {
                    return field.unset(ui, value);
                };
                let variants = library
                    .enum_type(enum_id)
                    .expect("an admitted enum literal's type is registered")
                    .variants();
                let current = variants
                    .iter()
                    .find(|v| *v == cur)
                    .expect("an admitted enum literal names a registered variant");
                let picked = field.combo(
                    ui,
                    DropdownOptions {
                        options: variants,
                        label: String::as_str,
                        current,
                    },
                )?;
                Some(ConstValue::Enum(picked.clone()))
            }
            // No literal form (pick-or-wire ports carry variants, handled above).
            DataType::Custom(_) => field.unset(ui, value),
        }
    }
}

impl Field<'_> {
    /// A dropdown over `pick`'s options, returning the newly picked option, or `None` when the
    /// selection is unchanged. Both pickers (the value-variant override and the `Enum` port) are
    /// this widget over different option lists, each read through its own `label`.
    ///
    /// Generic over the row, so no `Vec<&str>` of labels is projected on every frame.
    fn combo<'o, S: PartialEq>(self, ui: &mut Ui, pick: DropdownOptions<'o, S>) -> Option<&'o S> {
        let DropdownOptions {
            options,
            label,
            mut current,
        } = pick;
        let changed = ComboBox::labeled(&mut current, options.iter(), |option| label(option))
            .id(self.id)
            .button_style(&self.theme.button)
            .size(SizeSpec::FILL)
            .min_size((self.theme.width, 0.0))
            .show(ui)
            .changed;
        changed.then_some(current)
    }

    /// Editor for an untyped (`Any`) port: one text field that reinterprets what the user types
    /// into the tightest [`ConstValue`]: `true`/`false` → `Bool`, an integer → `Int`, a finite
    /// decimal → `Float`, anything else → `String`. The kind rides on the value itself, so
    /// `"42"` always reads back as `Int`. Returns the reinterpreted value only when the edit
    /// committed a change: see [`any_edit`].
    fn any_smart(self, ui: &mut Ui, value: &ConstValue) -> Option<ConstValue> {
        let shown = fmt!(ui, "{}", AnyText(value));
        any_edit(&self.text(ui, shown)?, value)
    }

    /// An optional input left unset: `Null`, the one admitted literal outside
    /// its port's coercion class. See [`Self::empty`]. Always returns `None`.
    fn unset(self, ui: &mut Ui, value: &ConstValue) -> Option<ConstValue> {
        debug_assert_eq!(
            *value,
            ConstValue::Null,
            "an admitted literal outside its port's class is `Null`"
        );
        self.empty(ui);
        None
    }

    /// A constant shown as written and not edited, in a box with an editor's resting look and
    /// size: the value of a type the library does not register. It senses nothing, so a click
    /// falls through to the surrounding row.
    fn read_only(self, ui: &mut Ui, shown: ShownValue<'_>) {
        let Self { theme, id } = self;
        let editor = &theme.field;
        let text = fmt!(ui, "{shown}");
        Panel::hstack()
            .id(id)
            .size(SizeSpec::FILL)
            .min_size((theme.width, 0.0))
            .padding(editor.defaults.padding)
            .child_align(VAlign::Center)
            .chrome(editor.looks.normal.chrome.clone())
            .show(ui, |ui| {
                Text::new(text)
                    .color(theme.unset)
                    .text_wrap(TextWrap::EllipsisMiddle)
                    .show(ui);
            });
    }

    /// An input with nothing in it — left unset, or bound to nothing: [`UNSET`] in the muted ink,
    /// in a box with an editor's resting look and size, as the inspector reads an input with
    /// nothing in it. It senses nothing, so a click falls through to the surrounding row.
    pub(crate) fn empty(self, ui: &mut Ui) {
        let Self { theme, id } = self;
        let editor = &theme.field;
        Panel::hstack()
            .id(id)
            .size(SizeSpec::FILL)
            .min_size((theme.width, 0.0))
            .padding(editor.defaults.padding)
            .child_align(VAlign::Center)
            .chrome(editor.looks.normal.chrome.clone())
            .show(ui, |ui| {
                Text::new(UNSET).color(theme.unset).show(ui);
            });
    }

    /// A buffered `TextEdit` over `current`: the draft palantir keeps under the field's id, handed
    /// back on the frame an edit commits it with a change (Enter, or focus left the field), `None`
    /// on every other — Escape included, which puts `current` back.
    fn text<'t>(self, ui: &mut Ui, current: impl Into<TextInput<'t>>) -> Option<String> {
        let Self { theme, id } = self;
        TextEdit::buffered(current)
            .id(id)
            .style(&theme.field)
            .size(SizeSpec::FILL)
            .min_size((theme.width, 0.0))
            .show(ui)
            .committed_text
    }

    /// Editor for an `Int` const: an editable `DragValue`. The draft re-seeds from the document
    /// value every frame and is emitted only on the widget's `committed` frame, which carries
    /// the gesture's final value, so one scrub or typed entry lands as one change. A span with
    /// both ends shows how far along it the value sits, as palantir's range fill.
    fn int(self, ui: &mut Ui, cur: i64, span: Option<IntSpan>) -> Option<ConstValue> {
        let mut draft = cur;
        let mut value = DragValue::new(&mut draft)
            .speed(int_speed(cur))
            .style(&self.theme.number)
            .text_align(Align::LEFT)
            .range_fill(true)
            .size(SizeSpec::FILL)
            .min_size((self.theme.width, 0.0))
            .id(self.id);
        if let Some(span) = span {
            value = value.range(int_end(span.min())..=span.max().map_or(f64::INFINITY, int_end));
        }
        let committed = value.show(ui).committed;
        (committed && draft != cur).then_some(ConstValue::Int(draft))
    }

    /// `Float` sibling of [`Self::int`].
    fn float(self, ui: &mut Ui, cur: f64, span: Option<FloatSpan>) -> Option<ConstValue> {
        let mut draft = cur;
        let mut value = DragValue::new(&mut draft)
            .speed(float_speed(cur))
            .decimals(FLOAT_DECIMALS)
            .style(&self.theme.number)
            .text_align(Align::LEFT)
            .range_fill(true)
            .size(SizeSpec::FILL)
            .min_size((self.theme.width, 0.0))
            .id(self.id);
        if let Some(span) = span {
            value = value.range(span.min()..=span.max().unwrap_or(f64::INFINITY));
        }
        let committed = value.show(ui).committed;
        // Bit-exact, as `ConstValue`'s `PartialEq` is, so `1.0` → `1` (same value, different
        // text) emits no change.
        (committed && draft.to_bits() != cur.to_bits()).then_some(ConstValue::Float(draft))
    }
}

/// An integer span's end as the `f64` a field's range reads. An end past ±2^53 rounds to the
/// nearest float, where a field's figures could not show the difference anyway.
#[expect(
    clippy::cast_precision_loss,
    reason = "an end past 2^53 rounds, below what a field shows"
)]
const fn int_end(end: i64) -> f64 {
    end as f64
}

/// Canonical text for an `Any` const, chosen so [`parse_any`] round-trips the numeric and bool
/// kinds: `Float` keeps its `.0` (else `3.0` would reparse as `Int`) and `Null` shows blank. A
/// `String` prints verbatim, so a numeric-looking string (`"42"`) is the one kind that doesn't
/// round-trip, the accepted ambiguity of an untyped literal.
#[derive(Clone, Copy, Debug)]
struct AnyText<'a>(&'a ConstValue);

impl Display for AnyText<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std_fmt::Result {
        shown_value::write_untyped(self.0, f)
    }
}

/// What committing `text` in an `Any` field showing `value` sets it to, or `None` when it reads
/// back as `value`. The field hands over only a text that differs from the one it showed, as
/// [`AnyText`] and [`parse_any`] do not round-trip every value: `Null` shows blank, which reads
/// back as an empty `String`, and a numeric `String` reads back as a number.
fn any_edit(text: &str, value: &ConstValue) -> Option<ConstValue> {
    let parsed = parse_any(text);
    (parsed != *value).then_some(parsed)
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

/// Drag speed for a float: ≈1% of the value's magnitude per logical pixel (floored at a unit's
/// worth). Sampled by `DragValue` at drag start, so it stays fixed for the duration of a drag.
fn float_speed(v: f64) -> f64 {
    v.abs().max(1.0) * 0.01
}

/// Drag speed for an integer: same magnitude-relative scaling, floored at
/// `0.25`/px so small counts stay adjustable (~4 px per step near zero).
fn int_speed(v: i64) -> f64 {
    ((v.unsigned_abs() as f64) * 0.01).max(0.25)
}

#[cfg(test)]
mod tests;
