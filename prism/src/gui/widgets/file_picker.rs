use std::path::Path;

use egui::{Align2, Color32, Pos2, Response, Sense, Stroke, StrokeKind, pos2, vec2};

use crate::common::StableId;
use crate::gui::Gui;
use crate::gui::style::{ButtonStyle, DragValueStyle};
use crate::gui::widgets::button::Button;

/// What kind of filesystem entry the picker accepts.
#[derive(Debug, Clone, Copy)]
pub enum FilePickerMode {
    ExistingFile,
    NewFile,
    Directory,
}

#[derive(Debug)]
#[must_use = "FilePicker does nothing until .show() is called"]
pub struct FilePicker<'a> {
    id: StableId,
    path: &'a mut String,
    extensions: &'a [String],
    mode: FilePickerMode,
    pos: Pos2,
    anchor: Align2,
    style: Option<DragValueStyle>,
}

impl<'a> FilePicker<'a> {
    pub fn new(
        id: StableId,
        path: &'a mut String,
        extensions: &'a [String],
        mode: FilePickerMode,
    ) -> Self {
        Self {
            id,
            path,
            extensions,
            mode,
            pos: Pos2::ZERO,
            anchor: Align2::RIGHT_CENTER,
            style: None,
        }
    }

    pub fn pos(mut self, pos: Pos2) -> Self {
        self.pos = pos;
        self
    }

    pub fn anchor(mut self, anchor: Align2) -> Self {
        self.anchor = anchor;
        self
    }

    pub fn style(mut self, style: DragValueStyle) -> Self {
        self.style = Some(style);
        self
    }

    pub fn show(self, gui: &mut Gui<'_>) -> Response {
        let id = self.id;
        let display_name = Path::new(self.path.as_str())
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "(none)".to_string());

        let padding = gui.style.small_padding;
        let font = gui.style.sub_font.clone();
        let text_color = gui.style.text_color;
        let style = self
            .style
            .unwrap_or_else(|| gui.style.node.const_bind_style.clone());

        let browse_text = "browse";
        let browse_galley = gui.ui_raw().painter().layout_no_wrap(
            browse_text.to_string(),
            font.clone(),
            text_color,
        );
        let browse_text_size = browse_galley.size();

        let filename_galley =
            gui.ui_raw()
                .painter()
                .layout_no_wrap(display_name.clone(), font.clone(), text_color);
        let filename_text_size = filename_galley.size();

        let inner_height = filename_text_size.y.max(browse_text_size.y);
        let button_inner_padding = gui.style.padding;
        let total_size = vec2(
            padding
                + filename_text_size.x
                + padding
                + button_inner_padding
                + browse_text_size.x
                + button_inner_padding,
            inner_height + padding * 2.0,
        );

        let rect = self.anchor.anchor_size(self.pos, total_size);

        // Calculate separator position
        let separator_x = rect.min.x + padding + filename_text_size.x + padding;

        // Browse button rect (right side, inside the main rect)
        let browse_rect =
            egui::Rect::from_min_max(pos2(separator_x, rect.min.y), rect.max).shrink(padding);

        // Draw outer background for entire widget
        gui.painter().rect(
            rect,
            style.radius,
            style.fill,
            style.stroke,
            StrokeKind::Outside,
        );

        // Draw filename text
        let filename_pos = pos2(
            rect.min.x + padding,
            rect.center().y - filename_text_size.y * 0.5,
        );
        gui.painter()
            .galley(filename_pos, filename_galley, text_color);

        let browse_response = Button::new(StableId::from_id(id.id().with("browse")))
            .text(browse_text)
            .font(font)
            .rect(browse_rect)
            .show(gui);

        if browse_response.clicked() {
            let mut dialog = rfd::FileDialog::new();
            if !self.extensions.is_empty() {
                let extensions: Vec<&str> = self.extensions.iter().map(|s| s.as_str()).collect();
                dialog = dialog.add_filter("Allowed files", &extensions);
            }
            let selected_path = match self.mode {
                FilePickerMode::ExistingFile => dialog.pick_file(),
                FilePickerMode::NewFile => dialog.save_file(),
                FilePickerMode::Directory => dialog.pick_folder(),
            };
            if let Some(selected_path) = selected_path {
                *self.path = selected_path.to_string_lossy().to_string();
            }
        }

        // Return response for the entire widget rect, not just the browse button
        let widget_response = gui
            .ui_raw()
            .interact(rect, browse_response.id, Sense::hover());
        widget_response | browse_response
    }
}
