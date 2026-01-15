use egui::{Align, Color32, Key, Order, Pos2, Stroke, Vec2};
use graph::function::Func;
use graph::prelude::FuncLib;

use crate::common::area::Area;
use crate::common::button::{Button, ButtonBackground};
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
                    gui.ui().set_max_width(150.0);

                    let mut categories: Vec<&str> =
                        func_lib.funcs.iter().map(|f| f.category.as_str()).collect();
                    categories.sort();
                    categories.dedup();

                    for category in categories {
                        Expander::new(category).show(gui, |gui| {
                            // Frame::none().inner_margin(10).show(gui, |gui| {
                            for func in func_lib.funcs.iter() {
                                if func.category != category {
                                    continue;
                                }
                                let btn_size = Vec2::new(gui.ui().available_width(), 20.0);
                                if Button::default()
                                    .background(ButtonBackground {
                                        disabled_fill: Color32::TRANSPARENT,
                                        idle_fill: Color32::TRANSPARENT,
                                        hover_fill: gui.style.hover_bg_fill,
                                        active_fill: gui.style.active_bg_fill,
                                        checked_fill: Color32::TRANSPARENT,
                                        inactive_stroke: Stroke::NONE,
                                        hovered_stroke: Stroke::NONE,
                                        radius: 0.0,
                                    })
                                    .size(btn_size)
                                    .text(&func.name)
                                    .align(Align::Min)
                                    .show(gui)
                                    .clicked()
                                {
                                    selected_func = Some(func);
                                    self.open = false;
                                }
                            }
                            // });
                        });
                    }
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
