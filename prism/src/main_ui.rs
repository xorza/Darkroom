use std::rc::Rc;

use crate::common::StableId;
use crate::gui::Gui;
use crate::gui::frame_output::RunCommand;
use crate::gui::graph_ui::GraphUi;
use crate::gui::log_ui::LogUi;
use crate::gui::style::Style;
use crate::gui::widgets::{Button, ListItem, Panel, PopupMenu};
use crate::input::InputSnapshot;
use crate::session::Session;
use eframe::egui;
use egui::{ViewportCommand, vec2};

#[derive(Clone, Debug)]
pub struct UiContext {
    ctx: egui::Context,
}

impl UiContext {
    pub fn new(ctx: &egui::Context) -> Self {
        Self { ctx: ctx.clone() }
    }

    pub fn request_redraw(&self) {
        self.ctx.request_repaint();
    }
    pub fn close_app(&self) {
        self.ctx.send_viewport_cmd(ViewportCommand::Close);
    }
}

#[derive(Debug)]
pub struct MainUi {
    pub graph_ui: GraphUi,
    pub log_ui: LogUi,
    pub ui_context: UiContext,
    /// Reference `Style` at scale=1.0, loaded from `style.toml` on
    /// startup. Serves as the canonical source for [`Gui::new`] every
    /// frame.
    pub style: Rc<Style>,

    pub arena: bumpalo::Bump,
}

impl MainUi {
    pub fn new(ui_context: UiContext) -> Self {
        let style = Style::from_file("style.toml").unwrap_or_default();
        Self {
            graph_ui: GraphUi::default(),
            log_ui: LogUi,
            ui_context,
            style: Rc::new(style),
            arena: bumpalo::Bump::new(),
        }
    }

    fn empty(&mut self, session: &mut Session) {
        self.graph_ui = GraphUi::default();
        session.empty_graph();
    }

    fn save(&mut self, session: &mut Session) {
        if let Some(path) = session.state.config.current_path.clone() {
            session.save_graph(&path);
        } else {
            self.save_as(session);
        }
    }

    fn save_as(&mut self, session: &mut Session) {
        let file = rfd::FileDialog::new()
            .add_filter("Lua", &["lua"])
            .add_filter("Scn", &["scn"])
            .add_filter("JSON", &["json"])
            .add_filter("Lz4 compressed Lua", &["lz4"])
            .save_file();

        if let Some(path) = file {
            session.save_graph(&path);
        }
    }

    pub fn load(&mut self, session: &mut Session) {
        let file = rfd::FileDialog::new()
            .add_filter("All supported", &["lua", "json", "scn", "lz4"])
            .add_filter("Lua", &["lua"])
            .add_filter("Scn", &["scn"])
            .add_filter("JSON", &["json"])
            .add_filter("Lz4 compressed Lua", &["lz4"])
            .pick_file();

        if let Some(path) = file {
            self.graph_ui = GraphUi::default();
            session.load_graph(&path);
        }
    }

    pub fn render(&mut self, session: &mut Session, gui: &mut Gui<'_>) {
        let input = gui.input_snapshot();

        session.update_shared_status();

        self.handle_shortcuts(&input, session);

        Panel::top(StableId::new("top_panel"))
            .show_separator_line(false)
            .show(gui, |gui| {
                gui.horizontal(|gui| {
                    self.file_menu(session, gui);
                });
            });

        Panel::bottom(StableId::new("status_panel"))
            .show_separator_line(false)
            .no_frame()
            .show(gui, |gui| {
                self.log_ui.render(gui, &session.state.status);
            });

        Panel::central().no_frame().show(gui, |gui| {
            self.graph_ui.render(gui, session, &input, &self.arena)
        });

        session.handle_output(self.graph_ui.output());
        self.arena.reset();
    }

    /// Top-level "File" menu: button that anchors a popup with the
    /// usual New / Save / Save as / Open / Exit entries. No raw egui
    /// chrome — `Button` + `PopupMenu` + `ListItem` carry the whole
    /// thing. `PopupMenu::close_on_click` (default true) closes the
    /// dropdown whenever any entry fires.
    ///
    /// All styling — font, button preset, padding, popup width —
    /// lives on [`MenuStyle`] (`gui.style.menu`). Entries are all
    /// forced to `menu.popup_min_width` so hover highlights the
    /// whole row, not just the text.
    fn file_menu(&mut self, session: &mut Session, gui: &mut Gui<'_>) {
        let menu = gui.style.menu.clone();
        let file_btn = Button::new(StableId::new("menu_file"))
            .text("File")
            .font(menu.font.clone())
            .background(menu.button)
            .padding(menu.padding)
            .show(gui);

        let item_size = vec2(
            menu.popup_min_width,
            gui.font_height(&menu.font) + menu.padding.y * 2.0,
        );
        let entry = |gui: &mut Gui<'_>, id: &'static str, label: &str| -> bool {
            ListItem::from_str(StableId::new(id), label)
                .font(menu.font.clone())
                .style(menu.button)
                .size(item_size)
                .show(gui)
                .clicked()
        };

        PopupMenu::new(&file_btn, "menu_file_popup")
            .min_width(menu.popup_min_width)
            .show(gui, |gui| {
                if entry(gui, "menu_file_new", "New") {
                    self.empty(session);
                }
                if entry(gui, "menu_file_save", "Save") {
                    self.save(session);
                }
                if entry(gui, "menu_file_save_as", "Save as") {
                    self.save_as(session);
                }
                if entry(gui, "menu_file_open", "Open") {
                    self.load(session);
                }
                if entry(gui, "menu_file_exit", "Exit") {
                    self.ui_context.close_app();
                }
            });
    }

    fn handle_shortcuts(&mut self, input: &InputSnapshot, session: &mut Session) {
        if input.cmd_only(egui::Key::Z) {
            self.graph_ui.cancel_gesture();
            session.undo(self.graph_ui.output());
        } else if input.cmd_shift(egui::Key::Z) {
            self.graph_ui.cancel_gesture();
            session.redo();
        }

        if input.cmd_shift(egui::Key::S) {
            self.save_as(session);
        } else if input.cmd_only(egui::Key::S) {
            self.save(session);
        } else if input.cmd(egui::Key::O) {
            self.load(session);
        }

        if input.cmd_shift(egui::Key::Space) {
            let output = self.graph_ui.output();
            output.set_run_cmd(if session.state.autorun {
                RunCommand::StopAutorun
            } else {
                RunCommand::StartAutorun
            });
        } else if input.cmd_only(egui::Key::Space) {
            self.graph_ui.output().set_run_cmd(RunCommand::RunOnce);
        }

        if input.cmd(egui::Key::Q) {
            self.ui_context.close_app();
        }
    }
}
