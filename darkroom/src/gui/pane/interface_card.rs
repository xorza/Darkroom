//! The inspector's interface card: a subgraph's ports, each renamed, retyped, moved, removed
//! and, for an input, given a default, from an instance or a boundary node of it.
//!
//! - **The interface is edited where its ports show**, beside the node that shows them, as
//!   Blender edits a node group's sockets in the sidebar of the node that holds the group. Each
//!   edit is one [`SubgraphIntent::SetInterface`], so it drops the wires the new interface does
//!   not hold, in every graph, as one history entry. Refused: a dialog of its own, a mode to
//!   enter and leave.

use palantir::WidgetId;
use palantir::{
    Checkbox, ComboBox, Configure, Panel, SizeSpec, Sizing, Spacing, Text, TextEdit, Ui, VAlign,
};
use scenarium::{ConstValue, DataType, EventDecl, GraphSet, Library, Numbered, PortId, SubgraphId};

use crate::core::edit::port_edit::{InterfaceSide, PortEdit, Shift};
use crate::core::edit::subgraph_intent::SubgraphIntent;
use crate::gui::frame::requests::Requests;
use crate::gui::pane::graph_ui::node_ui::const_editor::{ConstEditor, Field};
use crate::gui::theme::Theme;
use crate::gui::widgets::link::Link;
use crate::gui::widgets::support;

/// The scalar types a port can take, beside the library's own.
const SCALARS: [DataType; 6] = [
    DataType::Any,
    DataType::Bool,
    DataType::Int,
    DataType::Float,
    DataType::String,
    DataType::Dir,
];

/// What the interface card shows: one subgraph's interface, and whether the library shares it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct InterfaceCard<'a> {
    pub(crate) theme: &'a Theme,
    pub(crate) library: &'a Library,
    /// The set the subgraph is in, which names a type the library lacks as its file did.
    pub(crate) graphs: &'a GraphSet,
    pub(crate) subgraph: SubgraphId,
    pub(crate) shared: bool,
}

/// Record the interface card, raising the edits a frame's input makes of its subgraph.
pub(crate) fn show(ui: &mut Ui, card: InterfaceCard<'_>, out: &mut Requests) {
    let InterfaceCard {
        theme,
        library,
        graphs,
        subgraph,
        shared,
    } = card;
    let interface = &graphs
        .subgraph(subgraph)
        .expect("a card shows a subgraph the set holds")
        .interface;
    let mut edit = None;
    // The choices a port's type picks among: the scalars, then the library's types by name.
    let types: Vec<(DataType, &str)> = SCALARS
        .into_iter()
        .chain(library.data_types())
        .map(|data_type| (data_type, library.type_name(data_type)))
        .collect();
    Panel::vstack()
        .id(card_wid())
        .size(SizeSpec::FILL_W)
        .padding(Spacing::xy(theme.space.md, theme.space.sm))
        .gap(theme.space.xs)
        .chrome(theme.card_chrome())
        .show(ui, |ui| {
            Text::new("Interface")
                .style(&support::caption_text(ui, theme))
                .show(ui);
            let mut marked = shared;
            if Checkbox::new(&mut marked)
                .id(shared_wid())
                .label("Shared in the library")
                .show(ui)
                .changed
            {
                out.push_subgraph(SubgraphIntent::SetShared {
                    id: subgraph,
                    shared: marked,
                });
            }
            for (side, caption, ports) in [
                (
                    InterfaceSide::Inputs,
                    "Inputs",
                    interface
                        .inputs()
                        .iter()
                        .map(|input| PortRow {
                            port: input.id,
                            name: &input.name,
                            data: Some(RowData {
                                data_type: input.data_type,
                                type_name: graphs.type_name(library, input.data_type),
                                default: RowDefault::Input(input.default_value.as_ref()),
                            }),
                        })
                        .collect::<Vec<_>>(),
                ),
                (
                    InterfaceSide::Outputs,
                    "Outputs",
                    interface
                        .outputs()
                        .iter()
                        .map(|output| PortRow {
                            port: output.id,
                            name: &output.name,
                            data: Some(RowData {
                                data_type: output.data_type,
                                type_name: graphs.type_name(library, output.data_type),
                                default: RowDefault::Output,
                            }),
                        })
                        .collect(),
                ),
                (
                    InterfaceSide::Events,
                    "Events",
                    interface.events().iter().map(event_row).collect(),
                ),
            ] {
                let muted = support::muted_text(ui, theme, theme.font.caption);
                Panel::hstack()
                    .id_salt(("caption", caption))
                    .size(SizeSpec::FILL_W)
                    .gap(theme.space.xs)
                    .child_align(VAlign::Center)
                    .show(ui, |ui| {
                        Text::new(caption).style(&muted).show(ui);
                        // A data port is made by a wire dropped on a boundary node; an event
                        // carries no type for a wire to give, so it is added here.
                        if side == InterfaceSide::Events
                            && Link::new(port_wid(side, PortId::MIN, "add"), "+")
                                .tooltip("Add one")
                                .show(ui, theme)
                        {
                            let list = interface.events();
                            edit = Some(PortEdit::AddEvent {
                                port: PortId::mint(|id| list.iter().any(|event| event.id == id)),
                                name: format!("event {}", list.len() + 1),
                            });
                        }
                    });
                let last = ports.len().saturating_sub(1);
                for (idx, row) in ports.into_iter().enumerate() {
                    let edge = Edge {
                        first: idx == 0,
                        last: idx == last,
                    };
                    if let Some(made) = row.show(ui, theme, library, side, edge, &types) {
                        edit = Some(made);
                    }
                }
            }
            if let Some(made) = trigger_section(ui, theme, interface.trigger()) {
                edit = Some(made);
            }
        });
    if let Some(edit) = edit {
        out.push_subgraph(SubgraphIntent::SetInterface {
            id: subgraph,
            to: Box::new(edit.apply(interface)),
        });
    }
}

/// Whether a row is first or last in its list, where a move that way does nothing.
#[derive(Clone, Copy, Debug)]
struct Edge {
    first: bool,
    last: bool,
}

/// Whether a row's port takes a default: an input, which may hold one, or an output.
#[derive(Clone, Copy, Debug)]
enum RowDefault<'a> {
    Input(Option<&'a ConstValue>),
    Output,
}

/// One port's row: its name, a data port's type, its moves and its removal, and an input's
/// default.
#[derive(Debug)]
struct PortRow<'a> {
    port: PortId,
    name: &'a str,
    /// What a data port carries; `None` for an event, which carries no type.
    data: Option<RowData<'a>>,
}

/// A data port's type, and its default where it is an input.
#[derive(Debug)]
struct RowData<'a> {
    data_type: DataType,
    /// What the type reads where the picker lists no such choice: a type the library lacks.
    type_name: &'a str,
    default: RowDefault<'a>,
}

/// The trigger's caption, with its add while the interface has none, and its row: its name and
/// its removal, and no moves, as an interface has one trigger at most. The edit its input made
/// this frame, if any.
fn trigger_section(ui: &mut Ui, theme: &Theme, trigger: Option<&EventDecl>) -> Option<PortEdit> {
    let mut edit = None;
    let muted = support::muted_text(ui, theme, theme.font.caption);
    Panel::hstack()
        .id_salt(("caption", "Trigger"))
        .size(SizeSpec::FILL_W)
        .gap(theme.space.xs)
        .child_align(VAlign::Center)
        .show(ui, |ui| {
            Text::new("Trigger").style(&muted).show(ui);
            if trigger.is_none()
                && Link::new(trigger_wid("add"), "+")
                    .tooltip("Add the trigger an instance hears on its pin")
                    .show(ui, theme)
            {
                edit = Some(PortEdit::SetTrigger(Some(EventDecl::new("trigger"))));
            }
        });
    let Some(trigger) = trigger else {
        return edit;
    };
    let field = &theme.const_editor.rest;
    Panel::hstack()
        .id(trigger_wid("row"))
        .size(SizeSpec::FILL_W)
        .gap(theme.space.xs)
        .child_align(VAlign::Center)
        .show(ui, |ui| {
            if let Some(renamed) = TextEdit::buffered(&trigger.name)
                .id(trigger_wid("name"))
                .style(&field.field)
                .size((Sizing::fill(1.0), Sizing::HUG))
                .show(ui)
                .committed_text
                && renamed != trigger.name
            {
                edit = Some(PortEdit::SetTrigger(Some(EventDecl {
                    name: renamed,
                    ..trigger.clone()
                })));
            }
            if Link::new(trigger_wid("remove"), "\u{d7}")
                .tooltip("Remove the trigger, and every wire on the pin")
                .show(ui, theme)
            {
                edit = Some(PortEdit::SetTrigger(None));
            }
        });
    edit
}

/// An event's row.
fn event_row(event: &Numbered<EventDecl>) -> PortRow<'_> {
    PortRow {
        port: event.id,
        name: &event.name,
        data: None,
    }
}

impl PortRow<'_> {
    /// Record the row; the edit its input made this frame, if any.
    fn show(
        self,
        ui: &mut Ui,
        theme: &Theme,
        library: &Library,
        side: InterfaceSide,
        edge: Edge,
        types: &[(DataType, &str)],
    ) -> Option<PortEdit> {
        let Self { port, name, data } = self;
        let wid = |part: &'static str| port_wid(side, port, part);
        let field = &theme.const_editor.rest;
        let mut edit = None;
        Panel::hstack()
            .id(wid("row"))
            .size(SizeSpec::FILL_W)
            .gap(theme.space.xs)
            .child_align(VAlign::Center)
            .show(ui, |ui| {
                if let Some(renamed) = TextEdit::buffered(name)
                    .id(wid("name"))
                    .style(&field.field)
                    .size((Sizing::fill(1.0), Sizing::HUG))
                    .show(ui)
                    .committed_text
                    && renamed != name
                {
                    edit = Some(PortEdit::Rename {
                        side,
                        port,
                        name: renamed,
                    });
                }
                if let Some(data) = &data {
                    let mut picked = types
                        .iter()
                        .copied()
                        .find(|(option, _)| *option == data.data_type)
                        .unwrap_or((data.data_type, data.type_name));
                    if ComboBox::labeled(&mut picked, types.iter().copied(), |option| option.1)
                        .id(wid("type"))
                        .button_style(&field.button)
                        .show(ui)
                        .changed
                    {
                        edit = Some(PortEdit::Retype {
                            side,
                            port,
                            data_type: picked.0,
                        });
                    }
                }
                for (part, label, shift, at_edge) in [
                    ("earlier", "\u{2191}", Shift::Earlier, edge.first),
                    ("later", "\u{2193}", Shift::Later, edge.last),
                ] {
                    if !at_edge && Link::new(wid(part), label).show(ui, theme) {
                        edit = Some(PortEdit::Move { side, port, shift });
                    }
                }
                if Link::new(wid("remove"), "\u{d7}")
                    .tooltip("Remove this port, and every wire on it")
                    .show(ui, theme)
                {
                    edit = Some(PortEdit::Remove { side, port });
                }
            });
        if let Some(RowData {
            data_type,
            default: RowDefault::Input(default),
            ..
        }) = data
        {
            Panel::hstack()
                .id(wid("default_row"))
                .size(SizeSpec::FILL_W)
                .gap(theme.space.xs)
                .child_align(VAlign::Center)
                .show(ui, |ui| {
                    let muted = support::muted_text(ui, theme, theme.font.caption);
                    Text::new("default").style(&muted).show(ui);
                    match default {
                        Some(value) => {
                            let editor = ConstEditor {
                                field: Field {
                                    theme: field,
                                    id: wid("default"),
                                },
                                library,
                                value,
                                data_type,
                                value_variants: &[],
                            };
                            if let Some(to) = editor.show(ui) {
                                edit = Some(PortEdit::Default {
                                    port,
                                    value: Some(to),
                                });
                            }
                            if Link::new(wid("clear_default"), "clear").show(ui, theme) {
                                edit = Some(PortEdit::Default { port, value: None });
                            }
                        }
                        None => {
                            if let Some(seed) = data_type.default_value()
                                && Link::new(wid("set_default"), "set").show(ui, theme)
                            {
                                edit = Some(PortEdit::Default {
                                    port,
                                    value: Some(seed),
                                });
                            }
                        }
                    }
                });
        }
        edit
    }
}

fn shared_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.inspector.interface.shared")
}

fn card_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.inspector.interface")
}

/// The `part` of `port`'s row on `side`.
fn port_wid(side: InterfaceSide, port: PortId, part: &'static str) -> WidgetId {
    let side = match side {
        InterfaceSide::Inputs => "inputs",
        InterfaceSide::Outputs => "outputs",
        InterfaceSide::Events => "events",
    };
    WidgetId::from_hash(("darkroom.inspector.interface.port", side, port, part))
}

/// The `part` of the trigger's caption or row.
fn trigger_wid(part: &'static str) -> WidgetId {
    WidgetId::from_hash(("darkroom.inspector.interface.trigger", part))
}

#[cfg(test)]
mod tests {
    use scenarium::{
        DataType, EventDecl, InputDecl, Interface, Numbered, PortId, Ports, Subgraph, SubgraphId,
    };

    use crate::core::document::inspect_target::InspectTarget;
    use crate::core::document::internals::{DocumentFixture, insert_subgraph};
    use crate::core::document::item_id::ItemId;
    use crate::core::document::tab_ref::TabRef;
    use crate::core::edit::graph_intent::GraphIntent;
    use crate::core::edit::port_edit::InterfaceSide;
    use crate::gui::app::session::internals::SessionHarness;
    use crate::gui::pane::interface_card::{port_wid, trigger_wid};

    /// An instance's inspector shows its subgraph's ports, and a click on a port's removal
    /// takes it out of the interface, as one history entry. The trigger's add gives the
    /// interface its one trigger and goes, and its removal takes it away.
    #[test]
    fn the_inspector_removes_an_interface_port() {
        let mut fixture = DocumentFixture::default();
        let id = SubgraphId::from_u128(1);
        let (a, b) = (PortId::new(1), PortId::new(2));
        let interface = Interface::new(Ports {
            inputs: [(a, "a"), (b, "b")]
                .map(|(id, name)| Numbered::new(id, InputDecl::optional(name, DataType::Int)))
                .into(),
            outputs: vec![],
        });
        insert_subgraph(&mut fixture.document, id, Subgraph::new("inner", interface));
        let instance = fixture.add_instance(id);
        let mut h =
            SessionHarness::new(fixture.with_tab(TabRef::Inspector(InspectTarget::Selection)));
        h.apply(GraphIntent::SetSelection {
            to: [ItemId::Node(instance)].into(),
        });
        h.prime(3);
        let remove = port_wid(InterfaceSide::Inputs, a, "remove");
        assert!(
            h.ui.rect(remove).is_some(),
            "premise: the card shows the port"
        );

        h.ui.click(remove);
        h.frame();
        h.drain();
        let held = &h
            .session
            .open
            .document()
            .graphs
            .subgraph(id)
            .unwrap()
            .interface;
        assert_eq!(
            held.inputs()
                .iter()
                .map(|input| input.id)
                .collect::<Vec<_>>(),
            [b]
        );
        assert!(h.undo(), "the edit is one entry");
        let held = &h
            .session
            .open
            .document()
            .graphs
            .subgraph(id)
            .unwrap()
            .interface;
        assert_eq!(held.inputs().len(), 2);

        let trigger = |h: &SessionHarness| {
            h.session
                .open
                .document()
                .graphs
                .subgraph(id)
                .unwrap()
                .interface
                .trigger()
                .cloned()
        };
        assert_eq!(trigger(&h), None, "premise: no trigger");
        h.frame();
        h.ui.click(trigger_wid("add"));
        h.frame();
        h.drain();
        assert_eq!(trigger(&h), Some(EventDecl::new("trigger")));
        h.frame();
        assert!(
            h.ui.rect(trigger_wid("add")).is_none(),
            "one trigger at most"
        );
        h.ui.click(trigger_wid("remove"));
        h.frame();
        h.drain();
        assert_eq!(trigger(&h), None);
    }
}
