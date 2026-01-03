use eframe::egui;

use crate::ScenariumApp;
use crate::gui::graph::GraphUiAction;

pub fn render_main_window(app: &mut ScenariumApp, ctx: &egui::Context) {
    app.poll_compute_status();

    egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            {
                let style = ui.style_mut();
                style.spacing.button_padding = egui::vec2(16.0, 5.0);
                style.spacing.item_spacing = egui::vec2(10.0, 5.0);
                style
                    .text_styles
                    .entry(egui::TextStyle::Button)
                    .and_modify(|font| font.size = 18.0);
            }
            ui.menu_button("File", |ui| {
                {
                    let style = ui.style_mut();
                    style.spacing.button_padding = egui::vec2(16.0, 5.0);
                    style.spacing.item_spacing = egui::vec2(10.0, 5.0);
                    style
                        .text_styles
                        .entry(egui::TextStyle::Button)
                        .and_modify(|font| font.size = 18.0);
                }
                if ui.button("New").clicked() {
                    app.empty();
                    ui.close();
                }
                if ui.button("Save").clicked() {
                    app.save();
                    ui.close();
                }
                if ui.button("Load").clicked() {
                    app.load();
                    ui.close();
                }
                if ui.button("Test").clicked() {
                    app.test_graph();
                    ui.close();
                }
            });
        });
    });

    egui::CentralPanel::default().show(ctx, |ui| {
        app.graph_ui.render(
            ui,
            &mut app.view_graph,
            &app.func_lib,
            &mut app.graph_ui_interaction,
        );
    });

    egui::TopBottomPanel::bottom("status_panel").show(ctx, |ui| {
        ui.label(&app.status);
    });
    egui::TopBottomPanel::bottom("run_panel").show(ctx, |ui| {
        ui.horizontal(|ui| {
            if ui.button("Run").clicked() {
                app.run_graph();
            }
        });
    });

    if !app.graph_ui_interaction.actions.is_empty() {
        let node_ids_to_invalidate =
            app.graph_ui_interaction
                .actions
                .iter()
                .filter_map(|(node_id, graph_ui_action)| match graph_ui_action {
                    GraphUiAction::CacheToggled => None,
                    GraphUiAction::InputChanged | GraphUiAction::NodeRemoved => {
                        app.graph_updated = true;
                        Some(*node_id)
                    }
                });
        app.worker.invalidate_caches(node_ids_to_invalidate);
    }
}
