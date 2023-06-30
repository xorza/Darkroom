#![allow(dead_code)]
#![allow(unused_imports)]


// use uilib::app_base::run;
// use uilib::sample_app::SampleApp;
// use uilib::ui_app::UiApp;
//
// fn main() {
//     run::<UiApp>("UI App");
// }


use crate::app::NodeGraphExample;

mod app;
fn main() {
    eframe::run_native(
        "Nodeshop",
        eframe::NativeOptions::default(),
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(eframe::egui::Visuals::dark());

            Box::<NodeGraphExample>::default()
        }),
    )
        .expect("Failed to run native example");
}