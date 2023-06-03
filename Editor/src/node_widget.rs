use iced::Element;
use iced::widget::{Button, Column, Text};

pub(crate) struct NodeWidget {}

impl NodeWidget {
    pub fn new() -> Self {
        NodeWidget {}
    }

    pub fn view(&mut self) -> Element<()> {
        Column::new()
            .push(Text::new("Node"))
            .push(Button::new(Text::new("Connect")))
            .into()
    }
}
