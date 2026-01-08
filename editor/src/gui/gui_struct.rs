use std::marker::PhantomData;
use std::ptr::NonNull;

use egui::Ui;

use crate::gui::style::Style;

#[derive(Debug)]
pub struct Gui<'a> {
    ui: NonNull<Ui>,
    pub style: Style,
    _marker: PhantomData<&'a mut Ui>,
}

impl<'a> Gui<'a> {
    pub fn new(ui: &'a mut Ui, style: &Style) -> Self {
        Self {
            ui: NonNull::from(ui),
            style: style.clone(),
            _marker: PhantomData,
        }
    }

    pub fn ui(&mut self) -> &mut Ui {
        unsafe { self.ui.as_mut() }
    }
}
