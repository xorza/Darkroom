use std::path::Path;

use egui::{Align2, Pos2, Response, Sense, StrokeKind, Vec2, pos2, vec2};
use graph::data::{FsPathConfig, FsPathMode};

use crate::gui::Gui;
use crate::gui::style::DragValueStyle;

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

    pub fn show(self, gui: &mut Gui<'_>, id_salt: impl std::hash::Hash) -> Response {
        let display_name = Path::new(self.path.as_str())
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "(none)".to_string());

        let id = gui.ui().make_persistent_id(&id_salt);

        let padding = vec2(gui.style.small_padding, 0.0);
        let font = gui.style.sub_font.clone();
        let text_color = gui.style.text_color;
        let style = self
            .style
            .unwrap_or_else(|| gui.style.node.const_bind_style.clone());

        let browse_text = "...";
        let browse_galley =
            gui.ui()
                .painter()
                .layout_no_wrap(browse_text.to_string(), font.clone(), text_color);
        let browse_size = browse_galley.size() + padding * 2.0;

        let filename_galley =
            gui.ui()
                .painter()
                .layout_no_wrap(display_name.clone(), font, text_color);
        let filename_size = filename_galley.size() + padding * 2.0;

        let separator_width = 1.0;
        let total_size = vec2(
            filename_size.x + separator_width + browse_size.x,
            filename_size.y.max(browse_size.y),
        );

        let rect = self.align.anchor_size(self.pos, total_size);

        let filename_rect =
            egui::Rect::from_min_size(rect.min, vec2(filename_size.x, total_size.y));
        let browse_rect = egui::Rect::from_min_size(
            pos2(filename_rect.max.x + separator_width, rect.min.y),
            vec2(browse_size.x, total_size.y),
        );

        // Draw the combined background
        gui.painter().rect(
            rect,
            style.radius,
            style.fill,
            style.stroke,
            StrokeKind::Outside,
        );

        // Draw separator line
        gui.painter().vline(
            filename_rect.max.x + separator_width * 0.5,
            rect.y_range(),
            style.stroke,
        );

        // Draw filename text centered vertically
        let filename_text_pos = pos2(
            filename_rect.min.x + padding.x,
            filename_rect.center().y - filename_galley.size().y * 0.5,
        );
        gui.painter()
            .galley(filename_text_pos, filename_galley, text_color);

        // Draw browse text centered vertically
        let browse_text_pos = pos2(
            browse_rect.min.x + padding.x,
            browse_rect.center().y - browse_galley.size().y * 0.5,
        );
        gui.painter()
            .galley(browse_text_pos, browse_galley, text_color);

        let response = gui.ui().allocate_rect(rect, Sense::hover());
        let browse_response = gui
            .ui()
            .interact(browse_rect, id.with("browse_btn"), Sense::click());

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

        response | browse_response
    }
}
