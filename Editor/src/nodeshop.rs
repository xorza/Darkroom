use iced::{Alignment, Color, Element, Length, Sandbox, Theme};
use iced::widget::{Column, Container, container, text};
use iced::widget::container::Appearance;

use crate::graph_widget::NodeEditor;

#[derive(Default)]
pub(crate) struct Nodeshop {
    theme: Theme,

    node_editor: NodeEditor,
}

#[derive(Debug, Clone)]
pub(crate) enum Message {
    None,
}

impl Sandbox for Nodeshop {
    type Message = Message;

    fn new() -> Self {
        Nodeshop {
            theme: Theme::Dark,
            ..Nodeshop::default()
        }
    }

    fn title(&self) -> String {
        String::from("Nodeshop")
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::None => {}
        }
    }

    fn view(&self) -> Element<Message> {
        let children: Vec<Element<Message>> = vec![
            self.node_editor.view().map(|_| Message::None),
            text("Bezier tool example").width(Length::Shrink).into(),
        ];

        let column = Column::with_children(children)
            .padding(0)
            .spacing(0);

        Container::new(column)
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
            .into()
    }

    fn theme(&self) -> Theme {
        self.theme.clone()
    }
}
