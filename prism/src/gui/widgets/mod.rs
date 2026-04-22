// Widgets are small builder structs with public API surfaces (font,
// style, size, tooltip, etc.). Many builder methods aren't called
// internally today but exist as part of the widget's contract for
// future callers. `text_edit.rs` is additionally a vendored fork of
// egui's `TextEdit` and keeps methods for parity. Silencing
// dead_code at the module level rather than per-item.
#![allow(dead_code)]

pub mod area;
pub mod button;
pub mod column_flow;
pub mod combo_box;
pub mod drag_value;
pub mod expander;
pub mod file_picker;
pub mod frame;
pub mod hit_region;
pub mod image;
pub mod label;
pub mod layout;
pub mod panel;
pub mod popup_menu;
pub mod positioned_ui;
pub mod scroll_area;
pub mod separator;
pub mod space;
pub mod status_panel;
pub mod text_edit;
pub mod texture;

pub use area::Area;
pub use button::Button;
pub use column_flow::ColumnFlow;
pub use combo_box::ComboBox;
pub use drag_value::DragValue;
pub use expander::Expander;
pub use file_picker::{FilePicker, FilePickerMode};
pub use frame::Frame;
pub use hit_region::HitRegion;
pub use image::Image;
pub use label::Label;
pub use layout::Layout;
pub use panel::Panel;
pub use popup_menu::{ListItem, PopupMenu};
pub use positioned_ui::PositionedUi;
pub use scroll_area::ScrollArea;
pub use separator::Separator;
pub use space::Space;
pub use status_panel::StatusPanel;
pub use text_edit::TextEdit;
pub use texture::Texture;
