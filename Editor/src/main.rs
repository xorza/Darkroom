#![allow(dead_code)]
// #![allow(unused_imports)]

use crate::app::NodeshopApp;

mod app;
mod worker;
mod function_templates;
mod common;
mod eng_integration;
mod serialization;
mod arg_mapping;

fn main() {

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
