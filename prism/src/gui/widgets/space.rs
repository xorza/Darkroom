use crate::gui::Gui;

#[derive(Debug)]
pub struct Space {
    amount: f32,
}

impl Space {
    pub fn new(amount: f32) -> Self {
        Self { amount }
    }

    pub fn show(self, gui: &mut Gui<'_>) {
        gui.ui_raw().add_space(self.amount);
    }
}
