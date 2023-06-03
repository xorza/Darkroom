use iced::{Element, Length};
use iced::widget::text;

#[derive(Default)]
pub(crate) struct NodeEditor {}

#[derive(Debug, Clone)]
pub(crate) enum Message {
    None,
}


impl NodeEditor {
    pub fn view(&self) -> Element<Message> {
        text("Node Editor")
            .height(Length::Fill)
            .into()
    }
}