//! Project widget toolkit. Every widget builder finishes with `.show(gui, ..)`.
//!
//! Return-type contract:
//! - **`HitRegion`** has two terminals: `.interact()` returns
//!   [`egui::Response`]; `.interact_and_cull()` returns
//!   [`hit_region::HitOutput`] with rect + visibility for positioned widgets.
//! - **Atomic interactive** widgets (`Button`, `Label`, `Image`,
//!   `Separator`, `ComboBox`, `DragValue`, `FilePicker`, `TextEdit`, `ListItem`)
//!   return [`egui::Response`].
//! - **Container** widgets that take a body closure and have a meaningful outer
//!   response (`Area`, `Frame`, `Panel`, `Expander`) return
//!   [`egui::InnerResponse<R>`] (`Expander` uses `Option<R>` for its body since
//!   it skips it when collapsed).
//! - **`PopupMenu`** returns `Option<R>` — `None` means the popup wasn't open
//!   this frame, so the body never ran.
//! - **`ScrollArea`** returns `R` — egui's `ScrollAreaOutput` carries no
//!   `Response`, so there's nothing meaningful to wrap.
//! - **Self-contained renderers** with no external interaction value
//!   (`Space`, `StatusPanel`, `ColumnFlow`) return `()`.
//!
//! No widget should return raw `egui::Response`/`egui::InnerResponse` types
//! through the unprefixed alias only — always import via `use egui::{Response, …}`.

pub mod area;
pub mod button;
pub mod checkbox;
pub mod close_button;
pub mod column_flow;
pub mod combo_box;
pub mod constraints;
pub mod drag_value;
pub mod expander;
pub mod file_picker;
pub mod frame;
pub mod hit_region;
pub mod image;
pub mod label;
pub mod modal;
pub mod panel;
pub mod popup_menu;
pub mod radio_button;
pub mod scroll_area;
pub mod separator;
pub mod space;
pub mod status_panel;
pub mod text_edit;
pub mod texture;

pub use area::Area;
pub use button::Button;
pub use checkbox::Checkbox;
pub use close_button::CloseButton;
pub use column_flow::ColumnFlow;
pub use combo_box::ComboBox;
pub use constraints::Constraints;
pub use drag_value::DragValue;
pub use expander::Expander;
pub use file_picker::{FilePicker, FilePickerMode};
pub use frame::Frame;
pub use hit_region::HitRegion;
pub use image::Image;
pub use label::Label;
pub use modal::Modal;
pub use panel::Panel;
pub use popup_menu::{ListItem, PopupMenu};
pub use radio_button::RadioButton;
pub use scroll_area::ScrollArea;
pub use separator::Separator;
pub use space::Space;
pub use status_panel::StatusPanel;
pub use text_edit::TextEdit;
pub use texture::Texture;
