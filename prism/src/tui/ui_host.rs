use crate::ui_host::UiHost;

#[derive(Debug)]
pub struct TuiUiHost;

impl TuiUiHost {
    pub fn new() -> Self {
        Self
    }
}

impl UiHost for TuiUiHost {
    fn request_redraw(&self) {}
    fn close_app(&self) {}
}
