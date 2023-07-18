#![allow(dead_code)]
// #![allow(unused_imports)]

use crate::app::NodeshopApp;

mod app;

fn main() {
    let app = Box::<NodeshopApp>::default();

    eframe::run_native(
        "Nodeshop",
        eframe::NativeOptions::default(),
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(eframe::egui::Visuals::dark());

            app
        }),
    )
        .expect("Failed to run native example");
}
