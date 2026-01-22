use std::path::Path;

use egui::{Align2, Color32, Pos2, Response, Sense, Stroke, StrokeKind, pos2, vec2};
use scenarium::data::{FsPathConfig, FsPathMode};

use crate::common::button::Button;
use crate::gui::Gui;
use crate::gui::style::{ButtonStyle, DragValueStyle};

#[derive(Debug)]
pub struct FilePicker<'a> {
    path: &'a mut String,
    config: &'a FsPathConfig,
    pos: Pos2,
    align: Align2,
    style: Option<DragValueStyle>,
}

impl<'a> FilePicker<'a> {
    pub fn new(path: &'a mut String, config: &'a FsPathConfig) -> Self {
        Self {
            path,
            config,
            pos: Pos2::ZERO,
            align: Align2::RIGHT_CENTER,
            style: None,
        }
    }

    pub fn pos(mut self, pos: Pos2) -> Self {
        self.pos = pos;
        self
    }

    pub fn align(mut self, align: Align2) -> Self {
        self.align = align;
        self
    }

    pub fn style(mut self, style: DragValueStyle) -> Self {
        self.style = Some(style);
        self
    }

    pub fn show(self, gui: &mut Gui<'_>, _id_salt: impl std::hash::Hash) -> Response {
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
        let browse_galley =
            gui.ui()
                .painter()
                .layout_no_wrap(browse_text.to_string(), font.clone(), text_color);
        let browse_text_size = browse_galley.size();

        let filename_galley =
            gui.ui()
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

        let rect = self.align.anchor_size(self.pos, total_size);

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

        let browse_response = Button::default()
            .text(browse_text)
            .font(font)
            .rect(browse_rect)
            .show(gui);

        if browse_response.clicked() {
            let mut dialog = rfd::FileDialog::new();
            if !self.config.extensions.is_empty() {
                let extensions: Vec<&str> =
                    self.config.extensions.iter().map(|s| s.as_str()).collect();
                dialog = dialog.add_filter("Allowed files", &extensions);
            }
            let selected_path = match self.config.mode {
                FsPathMode::ExistingFile => dialog.pick_file(),
                FsPathMode::NewFile => dialog.save_file(),
                FsPathMode::Directory => dialog.pick_folder(),
            };
            if let Some(selected_path) = selected_path {
                *self.path = selected_path.to_string_lossy().to_string();
            }
        }

        // Return response for the entire widget rect, not just the browse button
        let widget_response = gui.ui().interact(rect, browse_response.id, Sense::hover());
        widget_response | browse_response
    }
}
