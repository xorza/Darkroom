#![allow(dead_code)]
// #![allow(unused_imports)]

use ::common::log_setup::setup_logging;

use crate::app::ScenariumApp;

mod app;
mod arg_mapping;
mod common;
mod eng_integration;
mod function_templates;
mod serialization;

fn main() {
    setup_logging("info");

    eframe::run_native(
        "Scenarium",
        eframe::NativeOptions::default(),
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(eframe::egui::Visuals::dark());
            let app = Box::new(ScenariumApp::new(cc));

            app
        }),
    )
        .expect("Failed to run native example");
}
