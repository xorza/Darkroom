use std::sync::Arc;

use bumpalo::Bump;
use bumpalo::collections::CollectIn;
use bumpalo::collections::Vec as BumpVec;
use egui::{Galley, Id, Order, Pos2, Sense, vec2};
use scenarium::function::Func;
use scenarium::prelude::FuncLib;

use crate::common::StableId;
use crate::common::area::Area;
use crate::common::column_flow::ColumnFlow;
use crate::common::expander::Expander;
use crate::common::frame::Frame;
use crate::common::popup_menu::ListItem;
use crate::gui::Gui;
use crate::input::InputSnapshot;

const POPUP_MIN_WIDTH: f32 = 150.0;
const POPUP_MIN_HEIGHT: f32 = 150.0;
const POPUP_MAX_HEIGHT: f32 = 300.0;
const BUTTON_MIN_WIDTH: f32 = 80.0;
const MAX_COLUMNS: usize = 2;

// === Types ===

#[derive(Debug)]
pub enum NewNodeSelection<'a> {
    Func { func: &'a Func, position: Pos2 },
    ConstBind,
}

#[derive(Debug, Default)]
enum NewNodeUiState {
    #[default]
    Closed,
    Open {
        position: Pos2,
        from_connection: bool,
    },
}

#[derive(Debug, Default)]
pub struct NewNodeUi {
    state: NewNodeUiState,
}

// === NewNodeUi ===

impl NewNodeUi {
    pub fn open(&mut self, position: Pos2) {
        self.state = NewNodeUiState::Open {
            position,
            from_connection: false,
        };
    }

    pub fn open_from_connection(&mut self, position: Pos2) {
        self.state = NewNodeUiState::Open {
            position,
            from_connection: true,
        };
    }

    pub fn close(&mut self) {
        self.state = NewNodeUiState::Closed;
    }

    pub fn is_open(&self) -> bool {
        matches!(self.state, NewNodeUiState::Open { .. })
    }

    pub fn show<'a>(
        &mut self,
        gui: &mut Gui<'_>,
        input: &InputSnapshot,
        func_lib: &'a FuncLib,
        arena: &Bump,
    ) -> Option<NewNodeSelection<'a>> {
        let NewNodeUiState::Open {
            position,
            from_connection,
        } = self.state
        else {
            return None;
        };

        // Capture interaction for the background
        let rect = gui.rect;
        gui.ui().interact(
            rect,
            Id::new("temp background for new node ui"),
            Sense::all(),
        );

        let mut selection: Option<NewNodeSelection<'a>> = None;

        let popup_response = show_popup(
            gui,
            position,
            from_connection,
            func_lib,
            arena,
            &mut selection,
        );

        if should_close_popup(input, &popup_response.response.rect) || selection.is_some() {
            self.close();
        }

        selection
    }
}

fn show_popup<'a>(
    gui: &mut Gui<'_>,
    position: Pos2,
    from_connection: bool,
    func_lib: &'a FuncLib,
    arena: &Bump,
    selection: &mut Option<NewNodeSelection<'a>>,
) -> egui::InnerResponse<()> {
    let popup_id = gui.ui().make_persistent_id("new_node_popup");

    Area::new(popup_id)
        .fixed_pos(position)
        .order(Order::Foreground)
        .show(gui, |gui| {
            Frame::popup(&gui.style.popup).sense(Sense::all()).show(
                gui,
                StableId::new("new_node_popup_frame"),
                |gui| {
                    gui.ui().set_min_width(POPUP_MIN_WIDTH);
                    gui.ui().set_min_height(POPUP_MIN_HEIGHT);
                    gui.ui().set_max_height(POPUP_MAX_HEIGHT);

                    gui.horizontal_justified(|gui| {
                        if from_connection {
                            show_const_bind_option(gui, selection);
                        }
                        show_function_categories(gui, position, func_lib, arena, selection);
                    });
                },
            );
        })
}

// === Helpers ===

fn should_close_popup(input: &InputSnapshot, popup_rect: &egui::Rect) -> bool {
    if input.escape_pressed() {
        return true;
    }

    if input.any_pointer_pressed
        && let Some(pos) = input.interact_pos
        && !popup_rect.contains(pos)
    {
        return true;
    }

    false
}

fn show_const_bind_option<'a>(gui: &mut Gui<'_>, selection: &mut Option<NewNodeSelection<'a>>) {
    gui.vertical(|gui| {
        let padding = gui.style.padding;
        let small_padding = gui.style.small_padding;
        let btn_font = gui.style.sub_font.clone();

        let button_width = BUTTON_MIN_WIDTH + padding * 2.0;
        let button_height = gui.font_height(&btn_font) + small_padding * 2.0;

        gui.ui().set_min_width(button_width);

        if ListItem::from_str("Const")
            .size(vec2(button_width, button_height))
            .show(gui, StableId::new("const_bind_option"))
            .clicked()
        {
            *selection = Some(NewNodeSelection::ConstBind);
        }
    });
}

fn show_function_categories<'a>(
    gui: &mut Gui<'_>,
    position: Pos2,
    func_lib: &'a FuncLib,
    arena: &Bump,
    selection: &mut Option<NewNodeSelection<'a>>,
) {
    let categories = collect_sorted_categories(func_lib, arena);

    for category in categories {
        gui.vertical(|gui| {
            Expander::new(category).default_open(true).show(gui, |gui| {
                show_category_functions(gui, position, func_lib, category, arena, selection);
            });
        });
    }
}

fn collect_sorted_categories<'a>(func_lib: &'a FuncLib, arena: &'a Bump) -> BumpVec<'a, &'a str> {
    let mut categories: BumpVec<&str> = BumpVec::new_in(arena);
    categories.extend(func_lib.funcs.iter().map(|f| f.category.as_str()));
    categories.sort();
    categories.dedup();
    categories
}

fn show_category_functions<'a>(
    gui: &mut Gui<'_>,
    position: Pos2,
    func_lib: &'a FuncLib,
    category: &str,
    arena: &Bump,
    selection: &mut Option<NewNodeSelection<'a>>,
) {
    let funcs: BumpVec<&Func> = func_lib
        .funcs
        .iter()
        .filter(|f| f.category == category)
        .collect_in(arena);

    let btn_font = gui.style.sub_font.clone();
    let mut galleys: BumpVec<Arc<Galley>> = BumpVec::new_in(arena);
    galleys.extend(funcs.iter().map(|func| {
        gui.painter()
            .layout_no_wrap(func.name.clone(), btn_font.clone(), gui.style.text_color)
    }));

    let max_width = galleys
        .iter()
        .map(|g| g.size().x)
        .fold(BUTTON_MIN_WIDTH, f32::max);

    let button_width = max_width + gui.style.padding * 2.0;
    let button_height = gui.font_height(&btn_font) + gui.style.small_padding * 2.0;

    ColumnFlow::new(button_width, button_height)
        .id(gui
            .ui()
            .make_persistent_id(("new_node_ui_funcs_category_scroll", category)))
        .max_columns(MAX_COLUMNS)
        .show(
            gui,
            galleys.iter().zip(funcs.iter()),
            |gui, (galley, func)| {
                let mut item = ListItem::from_galley(Arc::clone(galley))
                    .size(vec2(button_width, button_height));

                if let Some(tooltip) = func.description.as_ref() {
                    item = item.tooltip(tooltip);
                }

                if item
                    .show(gui, StableId::new(("func_btn", func.id)))
                    .clicked()
                {
                    *selection = Some(NewNodeSelection::Func { func, position });
                }
            },
        );
}
