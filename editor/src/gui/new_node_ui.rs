use egui::{Pos2, Vec2};
use graph::function::Func;
use graph::prelude::FuncLib;

use crate::gui::Gui;

#[derive(Debug, Default)]
pub struct NewNodeUi {
    open: bool,
    position: Pos2,
}

impl NewNodeUi {
    pub fn open(&mut self, position: Pos2) {
        self.open = true;
        self.position = position;
    }

    pub fn show<'a>(&mut self, gui: &mut Gui<'_>, func_lib: &'a FuncLib) -> Option<&'a Func> {
        if !self.open {
            return None;
        }

        let mut selected_func = None;

        let popup_id = gui.ui().make_persistent_id("new_node_popup");

        egui::Area::new(popup_id)
            .fixed_pos(self.position)
            .order(egui::Order::Foreground)
            .show(gui.ui().ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(150.0);

                    let mut categories: Vec<&str> =
                        func_lib.funcs.iter().map(|f| f.category.as_str()).collect();
                    categories.sort();
                    categories.dedup();

                    for category in categories {
                        ui.menu_button(category, |ui| {
                            for func in func_lib.funcs.iter() {
                                if func.category != category {
                                    continue;
                                }
                                if ui.button(&func.name).clicked() {
                                    selected_func = Some(func);
                                    self.open = false;
                                    ui.close();
                                }
                            }
                        });
                    }

                    ui.separator();
                    if ui.button("Cancel").clicked() {
                        self.open = false;
                    }
                });
            });

        if gui.ui().input(|i| i.key_pressed(egui::Key::Escape)) {
            self.open = false;
        }

        if selected_func.is_some() {
            self.open = false;
        }

        selected_func
    }

    pub fn position(&self) -> Pos2 {
        self.position
    }
}
