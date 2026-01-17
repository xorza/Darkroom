use egui::Id;

use crate::common::scroll_area::ScrollArea;
use crate::gui::Gui;

/// A layout control that renders items vertically, adding columns as needed when
/// content overflows the available height. Adds a scroll area if max columns are exceeded.
#[derive(Debug)]
pub struct ColumnFlow {
    id: Option<Id>,
    max_columns: usize,
    item_height: f32,
    item_width: f32,
}

impl ColumnFlow {
    pub fn new(item_width: f32, item_height: f32) -> Self {
        Self {
            id: None,
            max_columns: 2,
            item_height,
            item_width,
        }
    }

    pub fn id(mut self, id: Id) -> Self {
        self.id = Some(id);
        self
    }

    pub fn max_columns(mut self, max_columns: usize) -> Self {
        assert!(max_columns >= 1, "max_columns must be at least 1");
        self.max_columns = max_columns;
        self
    }

    /// Show the column flow layout with items provided by the iterator.
    /// The `add_item` callback is called for each item to render it.
    pub fn show<T, I>(self, gui: &mut Gui<'_>, items: I, mut add_item: impl FnMut(&mut Gui<'_>, &T))
    where
        I: ExactSizeIterator<Item = T>,
    {
        let item_count = items.len();
        if item_count == 0 {
            return;
        }

        let available_height = gui.ui().available_height();
        let max_items_per_column = (available_height / self.item_height).floor().max(1.0) as usize;

        // Determine number of columns needed
        let num_columns = if item_count <= max_items_per_column {
            1
        } else if item_count <= max_items_per_column * self.max_columns {
            // Items fit in multiple columns without scrolling
            item_count.div_ceil(max_items_per_column)
        } else {
            // Need scrolling, use max columns
            self.max_columns
        };

        // Distribute items evenly across columns
        let rows_per_column = item_count.div_ceil(num_columns);

        // Set max width to constrain the layout
        let total_width = self.item_width * num_columns as f32;
        gui.ui().set_max_width(total_width);

        // Collect items into a vec so we can chunk them
        let items_vec: Vec<T> = items.collect();

        let needs_scroll = item_count > max_items_per_column * self.max_columns;

        if needs_scroll {
            let mut scroll_area = ScrollArea::vertical();
            if let Some(id) = self.id {
                scroll_area = scroll_area.id(id);
            }
            scroll_area.show(gui, |gui| {
                Self::render_columns(gui, &items_vec, rows_per_column, &mut add_item);
            });
        } else {
            Self::render_columns(gui, &items_vec, rows_per_column, &mut add_item);
        }
    }

    fn render_columns<T>(
        gui: &mut Gui<'_>,
        items: &[T],
        rows_per_column: usize,
        add_item: &mut impl FnMut(&mut Gui<'_>, &T),
    ) {
        gui.horizontal(|gui| {
            for column_items in items.chunks(rows_per_column) {
                gui.vertical(|gui| {
                    for item in column_items {
                        add_item(gui, item);
                    }
                });
            }
        });
    }
}
