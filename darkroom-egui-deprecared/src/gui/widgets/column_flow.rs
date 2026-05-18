use crate::common::StableId;
use crate::gui::Gui;
use crate::gui::widgets::scroll_area::ScrollArea;

/// A layout control that renders items vertically, adding columns as needed when
/// content overflows the available height. Adds a scroll area if max columns are exceeded.
#[derive(Debug)]
#[must_use = "ColumnFlow does nothing until .show() is called"]
pub struct ColumnFlow {
    id: StableId,
    max_columns: usize,
    item_height: f32,
    item_width: f32,
}

impl ColumnFlow {
    pub fn new(id: StableId, item_width: f32, item_height: f32) -> Self {
        Self {
            id,
            max_columns: 2,
            item_height,
            item_width,
        }
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

        let available_height = gui.ui_raw().available_height();
        let layout = compute_layout(
            item_count,
            available_height,
            self.item_height,
            self.max_columns,
        );

        // Set max width to constrain the layout
        let total_width = self.item_width * layout.num_columns as f32;
        gui.ui_raw().set_max_width(total_width);

        // Collect items into a vec so we can chunk them
        let items_vec: Vec<T> = items.collect();

        let rows_per_column = layout.rows_per_column;
        if layout.needs_scroll {
            ScrollArea::vertical(self.id).show(gui, |gui| {
                Self::render_columns(gui, &items_vec, rows_per_column, &mut add_item);
            });
        } else {
            Self::render_columns(gui, &items_vec, rows_per_column, &mut add_item);
        }
    }
}

/// Result of laying out `item_count` items into columns under a height budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ColumnLayout {
    pub num_columns: usize,
    pub rows_per_column: usize,
    pub needs_scroll: bool,
}

/// Pure layout decision for [`ColumnFlow`]. One column for short lists; up to
/// `max_columns` columns to fit without scrolling; if even that is too narrow,
/// `max_columns` columns plus vertical scroll.
pub(crate) fn compute_layout(
    item_count: usize,
    available_height: f32,
    item_height: f32,
    max_columns: usize,
) -> ColumnLayout {
    assert!(max_columns >= 1, "max_columns must be at least 1");
    let max_items_per_column = (available_height / item_height).floor().max(1.0) as usize;

    let num_columns = if item_count <= max_items_per_column {
        1
    } else if item_count <= max_items_per_column * max_columns {
        item_count.div_ceil(max_items_per_column)
    } else {
        max_columns
    };

    ColumnLayout {
        num_columns,
        rows_per_column: item_count.div_ceil(num_columns.max(1)),
        needs_scroll: item_count > max_items_per_column * max_columns,
    }
}

impl ColumnFlow {
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

#[cfg(test)]
mod tests {
    use super::compute_layout;

    // available_height = 100, item_height = 20 → max_items_per_column = 5.

    #[test]
    fn fits_single_column_when_under_capacity() {
        let layout = compute_layout(3, 100.0, 20.0, 2);
        assert_eq!(layout.num_columns, 1);
        assert_eq!(layout.rows_per_column, 3);
        assert!(!layout.needs_scroll);
    }

    #[test]
    fn fills_to_capacity_in_single_column() {
        let layout = compute_layout(5, 100.0, 20.0, 2);
        assert_eq!(layout.num_columns, 1);
        assert_eq!(layout.rows_per_column, 5);
        assert!(!layout.needs_scroll);
    }

    #[test]
    fn spills_into_second_column_without_scroll() {
        // 6 items, 5 per column, max 2 cols → ceil(6/5)=2 cols, ceil(6/2)=3 rows.
        let layout = compute_layout(6, 100.0, 20.0, 2);
        assert_eq!(layout.num_columns, 2);
        assert_eq!(layout.rows_per_column, 3);
        assert!(!layout.needs_scroll);
    }

    #[test]
    fn engages_scroll_at_max_columns_overflow() {
        // 11 items, 5 per col, max 2 cols → exceeds 2*5=10 → scroll, 2 cols, ceil(11/2)=6 rows.
        let layout = compute_layout(11, 100.0, 20.0, 2);
        assert_eq!(layout.num_columns, 2);
        assert_eq!(layout.rows_per_column, 6);
        assert!(layout.needs_scroll);
    }

    #[test]
    fn tiny_height_clamps_capacity_to_one() {
        // available < item_height → floor() == 0, but max(1) keeps capacity at 1.
        let layout = compute_layout(3, 5.0, 20.0, 3);
        assert_eq!(layout.num_columns, 3);
        // 3 items split into 3 cols → 1 row each.
        assert_eq!(layout.rows_per_column, 1);
        assert!(!layout.needs_scroll);
    }

    #[test]
    fn max_columns_one_always_single_column() {
        let layout = compute_layout(50, 100.0, 20.0, 1);
        assert_eq!(layout.num_columns, 1);
        assert_eq!(layout.rows_per_column, 50);
        assert!(layout.needs_scroll);
    }

    #[test]
    #[should_panic(expected = "max_columns must be at least 1")]
    fn zero_max_columns_panics() {
        let _ = compute_layout(1, 100.0, 20.0, 0);
    }
}
