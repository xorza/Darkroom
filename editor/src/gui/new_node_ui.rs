use egui::{Align, Key, Order, Pos2, Vec2, vec2};
use graph::function::Func;
use graph::prelude::FuncLib;

use crate::common::area::Area;
use crate::common::button::{self, Button};
use crate::common::expander::Expander;
use crate::common::frame::Frame;
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

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn show<'a>(&mut self, gui: &mut Gui<'_>, func_lib: &'a FuncLib) -> Option<&'a Func> {
        if !self.open {
            return None;
        }

        let mut selected_func = None;

        let popup_id = gui.ui().make_persistent_id("new_node_popup");

        Area::new(popup_id)
            .fixed_pos(self.position)
            .order(Order::Foreground)
            .show(gui, |gui| {
                Frame::popup(&gui.style.popup).show(gui, |gui| {
                    gui.ui().set_min_width(150.0);
                    gui.ui().set_min_height(150.0);

                    gui.horizontal(|gui| {
                        let mut categories: Vec<&str> =
                            func_lib.funcs.iter().map(|f| f.category.as_str()).collect();
                        categories.sort();
                        categories.dedup();

                        for category in categories {
                            gui.vertical(|gui| {
                                gui.ui.set_min_width(80.0 + gui.style.padding * 2.0);

                                Expander::new(category).default_open(true).show(gui, |gui| {
                                    for func in func_lib.funcs.iter() {
                                        if func.category != category {
                                            continue;
                                        }

                                        let btn_font = gui.style.sub_font.clone();
                                        let max_width = func_lib
                                            .funcs
                                            .iter()
                                            .filter(|f| f.category == category)
                                            .map(|f| {
                                                gui.painter()
                                                    .layout_no_wrap(
                                                        f.name.clone(),
                                                        btn_font.clone(),
                                                        gui.style.text_color,
                                                    )
                                                    .size()
                                                    .x
                                            })
                                            .max_by(|a, b| a.partial_cmp(b).unwrap())
                                            .unwrap_or(80.0)
                                            .max(80.0);

                                        let button_width = max_width + gui.style.padding * 2.0;
                                        let button_height = gui.font_height(&btn_font)
                                            + gui.style.small_padding * 2.0;

                                        if Button::default()
                                            .background(gui.style.list_button)
                                            .text(&func.name)
                                            .size(vec2(button_width, button_height))
                                            .align(Align::Min)
                                            .show(gui)
                                            .clicked()
                                        {
                                            selected_func = Some(func);
                                            self.open = false;
                                        }
                                    }
                                });
                            });
                        }
                    });
                });
            });

        if gui.ui().input(|i| i.key_pressed(Key::Escape)) {
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
