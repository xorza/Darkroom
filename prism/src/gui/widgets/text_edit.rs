//! Thin wrapper around `egui::TextEdit`. Defaults the inner margin to
//! `gui.style.padding` so it tracks our zoom scale, and the text color
//! to `widgets.inactive.text_color()` (rather than egui's
//! response-aware default, which is too bright against our dark
//! theme). All other behavior — IME, paste/cut, undo, focus, scrolling
//! — is delegated to stock egui.
//!
//! Builder surface is intentionally minimal: only the methods our
//! callers (`drag_value`, `details::show_name_editor`) use today. Add
//! more as needed.

use std::hash::Hash;

use egui::{Align, Color32, FontSelection, Id, Margin, TextBuffer};

use crate::gui::Gui;

#[must_use = "TextEdit does nothing until .show() is called"]
pub struct TextEdit<'t> {
    inner: egui::TextEdit<'t>,
    margin_override: Option<Margin>,
    text_color_override: Option<Color32>,
}

impl<'t> TextEdit<'t> {
    pub fn singleline(text: &'t mut dyn TextBuffer) -> Self {
        Self::wrap(egui::TextEdit::singleline(text))
    }

    pub fn multiline(text: &'t mut dyn TextBuffer) -> Self {
        Self::wrap(egui::TextEdit::multiline(text))
    }

    fn wrap(inner: egui::TextEdit<'t>) -> Self {
        Self {
            inner,
            margin_override: None,
            text_color_override: None,
        }
    }

    pub fn id(mut self, id: Id) -> Self {
        self.inner = self.inner.id(id);
        self
    }

    pub fn id_salt(mut self, id_salt: impl Hash) -> Self {
        self.inner = self.inner.id_salt(id_salt);
        self
    }

    pub fn font(mut self, font: impl Into<FontSelection>) -> Self {
        self.inner = self.inner.font(font);
        self
    }

    pub fn text_color(mut self, color: Color32) -> Self {
        self.text_color_override = Some(color);
        self
    }

    pub fn desired_width(mut self, desired_width: f32) -> Self {
        self.inner = self.inner.desired_width(desired_width);
        self
    }

    pub fn horizontal_align(mut self, align: Align) -> Self {
        self.inner = self.inner.horizontal_align(align);
        self
    }

    pub fn vertical_align(mut self, align: Align) -> Self {
        self.inner = self.inner.vertical_align(align);
        self
    }

    pub fn clip_text(mut self, clip: bool) -> Self {
        self.inner = self.inner.clip_text(clip);
        self
    }

    pub fn margin(mut self, margin: impl Into<Margin>) -> Self {
        self.margin_override = Some(margin.into());
        self
    }

    /// Stock egui takes a full `Frame`; translate the bool form: `false`
    /// drops the frame, `true` keeps the egui default.
    pub fn frame(mut self, frame: bool) -> Self {
        if !frame {
            self.inner = self.inner.frame(egui::Frame::NONE);
        }
        self
    }

    pub fn char_limit(mut self, limit: usize) -> Self {
        self.inner = self.inner.char_limit(limit);
        self
    }

    pub fn show(self, gui: &mut Gui<'_>) -> egui::Response {
        let margin = self
            .margin_override
            .unwrap_or_else(|| gui.style.padding.into());
        let text_color = self
            .text_color_override
            .unwrap_or_else(|| gui.ui_raw().visuals().widgets.inactive.text_color());
        self.inner
            .margin(margin)
            .text_color(text_color)
            .show(gui.ui_raw())
            .response
            .response
    }
}
