#![allow(dead_code)]

use iced::{Application, Settings};

use crate::nodeshop::Nodeshop;

mod node_widget;
mod graph_widget;
mod nodeshop;


pub fn main() -> iced::Result {
    Nodeshop::run(Settings {
        antialiasing: true,
        ..Settings::default()
    })
}
