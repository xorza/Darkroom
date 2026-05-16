use std::sync::Arc;

use anyhow::Result;
use eframe::NativeOptions;

use crate::gui::app::GuiApp;

/// Entry point for the egui frontend. Spins up eframe with the darkroom-egui
/// window icon and bundled font, then hands control to [`GuiApp`].
pub fn run(launch_config: crate::launch_config::LaunchConfig) -> Result<()> {
    let options = NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_icon(load_window_icon())
            .with_app_id("darkroom-egui"),
        persist_window: true,
        ..Default::default()
    };

    eframe::run_native(
        "Darkroom",
        options,
        Box::new(move |cc| {
            configure_fonts(&cc.egui_ctx);
            Ok(Box::new(GuiApp::new(&cc.egui_ctx, launch_config)))
        }),
    )?;

    Ok(())
}

fn load_window_icon() -> Arc<egui::IconData> {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../assets/darkroom-egui.png"))
        .expect("window icon PNG should be a valid RGBA image");
    Arc::new(icon)
}

fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let font_data = egui::FontData::from_static(include_bytes!("../../assets/Raleway-Medium.ttf"));
    fonts
        .font_data
        .insert("Raleway".to_owned(), Arc::new(font_data));

    let proportional = fonts
        .families
        .get_mut(&egui::FontFamily::Proportional)
        .expect("proportional font family should exist in default font definitions");
    proportional.insert(0, "Raleway".to_owned());

    ctx.set_fonts(fonts);
}
