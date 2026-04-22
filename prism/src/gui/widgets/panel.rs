//! Unified wrapper around `egui::TopBottomPanel` and `egui::CentralPanel`.
//! Takes `&mut Gui` and hands a child `Gui` to the body closure — app
//! code never constructs a `Gui` from a raw `egui::Ui` itself.

use crate::common::StableId;
use crate::gui::Gui;

#[derive(Debug, Clone, Copy)]
enum PanelKind {
    Top(StableId),
    Bottom(StableId),
    Central,
}

#[derive(Debug)]
#[must_use = "Panel does nothing until .show() is called"]
pub struct Panel {
    kind: PanelKind,
    show_separator_line: bool,
    no_frame: bool,
}

impl Panel {
    pub fn top(id: StableId) -> Self {
        Self::new(PanelKind::Top(id))
    }

    pub fn bottom(id: StableId) -> Self {
        Self::new(PanelKind::Bottom(id))
    }

    /// The single central panel of a Ui. Has no id — one per parent
    /// Ui by construction.
    pub fn central() -> Self {
        Self::new(PanelKind::Central)
    }

    fn new(kind: PanelKind) -> Self {
        Self {
            kind,
            show_separator_line: true,
            no_frame: false,
        }
    }

    /// Whether to draw a 1px separator line between the panel and the
    /// central area. No-op on [`Panel::central`].
    pub fn show_separator_line(mut self, show: bool) -> Self {
        self.show_separator_line = show;
        self
    }

    /// Drop the default panel background/margin frame — body renders
    /// directly onto the panel's rect.
    pub fn no_frame(mut self) -> Self {
        self.no_frame = true;
        self
    }

    pub fn show<R>(self, gui: &mut Gui<'_>, body: impl FnOnce(&mut Gui<'_>) -> R) -> R {
        let style = gui.style.clone();
        let run = |ui: &mut egui::Ui| {
            let mut child = Gui::new(ui, &style);
            body(&mut child)
        };

        match self.kind {
            PanelKind::Top(id) => build_side(egui::Panel::top(id.id()), self, run, gui),
            PanelKind::Bottom(id) => build_side(egui::Panel::bottom(id.id()), self, run, gui),
            PanelKind::Central => {
                let panel = if self.no_frame {
                    egui::CentralPanel::default().frame(egui::Frame::NONE)
                } else {
                    egui::CentralPanel::default()
                };
                panel.show_inside(gui.ui_raw(), run).inner
            }
        }
    }
}

fn build_side<R>(
    panel: egui::Panel,
    cfg: Panel,
    run: impl FnOnce(&mut egui::Ui) -> R,
    gui: &mut Gui<'_>,
) -> R {
    let mut panel = panel.show_separator_line(cfg.show_separator_line);
    if cfg.no_frame {
        panel = panel.frame(egui::Frame::NONE);
    }
    panel.show_inside(gui.ui_raw(), run).inner
}
