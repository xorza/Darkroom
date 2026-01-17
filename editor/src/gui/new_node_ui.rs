use std::cmp::Ordering;
use std::sync::Arc;

use bumpalo::Bump;
use bumpalo::collections::Vec as BumpVec;
use egui::{Align, Galley, Key, Layout, Order, Pos2, vec2};
use graph::function::Func;
use graph::prelude::FuncLib;

use crate::common::area::Area;
use crate::common::button::Button;
use crate::common::expander::Expander;
use crate::common::frame::Frame;
use crate::common::scroll_area::ScrollArea;
use crate::gui::Gui;

/// Result of showing the new node UI
#[derive(Debug)]
pub enum NewNodeSelection<'a> {
    /// User selected a function to create a node
    Func(&'a Func),
    /// User selected to create a const binding (only available when opened from connection)
    ConstBind,
}

#[derive(Debug, Default)]
pub struct NewNodeUi {
    open: bool,
    position: Pos2,
    /// Whether this was opened from a connection drag (enables const bind option)
    from_connection: bool,
}

impl NewNodeUi {
    pub fn open(&mut self, position: Pos2) {
        self.open = true;
        self.position = position;
        self.from_connection = false;
    }

    /// Open the UI from a connection drag, which enables the const bind option
    pub fn open_from_connection(&mut self, position: Pos2) {
        self.open = true;
        self.position = position;
        self.from_connection = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.from_connection = false;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn show<'a>(
        &mut self,
        gui: &mut Gui<'_>,
        func_lib: &'a FuncLib,
        arena: &Bump,
    ) -> Option<NewNodeSelection<'a>> {
        if !self.open {
            return None;
        }

        let mut selection: Option<NewNodeSelection<'a>> = None;
        let from_connection = self.from_connection;

        let popup_id = gui.ui().make_persistent_id("new_node_popup");

        let popup_response = Area::new(popup_id)
            .fixed_pos(self.position)
            .order(Order::Foreground)
            .show(gui, |gui| {
                Frame::popup(&gui.style.popup).show(gui, |gui| {
                    gui.ui().set_min_width(150.0);
                    gui.ui().set_min_height(150.0);
                    gui.ui().set_max_height(300.0);

                    gui.horizontal_justified(|gui| {
                        // Show const bind option if opened from connection
                        if from_connection {
                            gui.vertical(|gui| {
                                let padding = gui.style.padding;
                                let small_padding = gui.style.small_padding;
                                gui.ui().set_min_width(80.0 + padding * 2.0);

                                let btn_font = gui.style.sub_font.clone();
                                let button_width = 80.0 + padding * 2.0;
                                let button_height =
                                    gui.font_height(&btn_font) + small_padding * 2.0;

                                if Button::default()
                                    .background(gui.style.list_button)
                                    .text("Const")
                                    .size(vec2(button_width, button_height))
                                    .align(Align::Min)
                                    .show(gui)
                                    .clicked()
                                {
                                    selection = Some(NewNodeSelection::ConstBind);
                                }
                            });
                        }

                        let mut categories: BumpVec<&str> = BumpVec::new_in(arena);
                        categories.extend(func_lib.funcs.iter().map(|f| f.category.as_str()));
                        categories.sort();
                        categories.dedup();

                        for category in categories {
                            gui.vertical(|gui| {
                                Expander::new(category).default_open(true).show(gui, |gui| {
                                    let mut funcs: BumpVec<&Func> = BumpVec::new_in(arena);
                                    funcs.extend(
                                        func_lib.funcs.iter().filter(|f| f.category == category),
                                    );

                                    let btn_font = gui.style.sub_font.clone();
                                    let mut galleys: BumpVec<Arc<Galley>> = BumpVec::new_in(arena);
                                    galleys.extend(funcs.iter().map(|&func| {
                                        gui.painter().layout_no_wrap(
                                            func.name.clone(),
                                            btn_font.clone(),
                                            gui.style.text_color,
                                        )
                                    }));

                                    const MIN_WIDTH: f32 = 80.0;
                                    const MAX_COLUMNS: usize = 2;

                                    let max_width = galleys
                                        .iter()
                                        .map(|galley| galley.size().x)
                                        .max_by(|&a, &b| {
                                            a.partial_cmp(&b).unwrap_or(Ordering::Equal)
                                        })
                                        .unwrap_or(MIN_WIDTH)
                                        .max(MIN_WIDTH);

                                    let button_width = max_width + gui.style.padding * 2.0;
                                    let button_height =
                                        gui.font_height(&btn_font) + gui.style.small_padding * 2.0;

                                    // Calculate how many items fit in available height
                                    let available_height = gui.ui().available_height();
                                    let max_items_per_column =
                                        (available_height / button_height).floor().max(1.0)
                                            as usize;

                                    // Use minimum columns needed to fit all items without scrolling
                                    let num_columns = if funcs.len() <= max_items_per_column {
                                        1
                                    } else if funcs.len() <= max_items_per_column * MAX_COLUMNS {
                                        // Items fit in multiple columns without scrolling
                                        MAX_COLUMNS
                                    } else {
                                        // Need scrolling anyway, use max columns
                                        MAX_COLUMNS
                                    };
                                    // Distribute items evenly across columns
                                    let rows_per_column = funcs.len().div_ceil(num_columns);

                                    // Limit width to actual number of columns used
                                    let total_width = button_width * num_columns as f32;
                                    gui.ui().set_max_width(total_width);

                                    ScrollArea::vertical()
                                        .id(gui.ui.make_persistent_id((
                                            "new_node_ui_funcs_category_scroll",
                                            category,
                                        )))
                                        .show(gui, |gui| {
                                            gui.horizontal(|gui| {
                                                let mut items: BumpVec<_> = BumpVec::new_in(arena);
                                                items.extend(galleys.iter().zip(&funcs));

                                                for column_items in items.chunks(rows_per_column) {
                                                    gui.vertical(|gui| {
                                                        for (galley, func) in column_items {
                                                            let mut btn = Button::default()
                                                                .background(gui.style.list_button)
                                                                .galley((*galley).clone())
                                                                .size(vec2(
                                                                    button_width,
                                                                    button_height,
                                                                ))
                                                                .align(Align::Min);
                                                            if let Some(tooltip) =
                                                                func.description.as_ref()
                                                            {
                                                                btn = btn.tooltip(tooltip);
                                                            }

                                                            if btn.show(gui).clicked() {
                                                                selection = Some(
                                                                    NewNodeSelection::Func(func),
                                                                );
                                                            }
                                                        }
                                                    });
                                                }
                                            });
                                        });
                                });
                            });
                        }
                    });
                });
            });

        // Check for clicks outside the popup
        if gui.ui().input(|i| i.pointer.any_pressed()) {
            let popup_rect = popup_response.response.rect;
            if let Some(pointer_pos) = gui.ui().input(|i| i.pointer.interact_pos())
                && !popup_rect.contains(pointer_pos)
            {
                self.close();
            }
        }

        if gui.ui().input(|i| i.key_pressed(Key::Escape)) {
            self.close();
        }

        if selection.is_some() {
            self.close();
        }

        selection
    }

    pub fn position(&self) -> Pos2 {
        self.position
    }
}
