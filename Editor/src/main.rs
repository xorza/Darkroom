#![allow(dead_code)]
// #![allow(unused_imports)]


use ::common::log_setup::setup_logging;

use crate::app::NodeshopApp;

mod app;
mod function_templates;
mod common;
mod eng_integration;
mod serialization;
mod arg_mapping;

fn main() {
    setup_logging("info");

    eframe::run_native(
        "Nodeshop",
        eframe::NativeOptions::default(),
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(eframe::egui::Visuals::dark());
            let app = Box::new(NodeshopApp::new(cc));

            app
        }),
    )
        .expect("Failed to run native example");
}
