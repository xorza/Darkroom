//! [`InputField`]: what an input's value cell holds, the one answer the node's row and the
//! inspector's row both draw from.
//!
//! - **One reading of an input, two hosts.** The node's port row and the inspector's row each
//!   match the same [`InputField`] and record a constant through the same [`record`], so a value
//!   edited in either lands one `SetInput`, as Blender's Sidebar edits the selected node's
//!   properties ([nodes](https://docs.blender.org/manual/en/latest/interface/controls/nodes/introduction.html)).
//!   Refused: a summary of its own in the inspector, which read the input again and drifted.

use palantir::Ui;
use scenarium::{Binding, ConstValue, GhostSource, OutputPort};

use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::frame::app_command::{AppCommand, EditCommand, PathPick};
use crate::gui::frame::requests::Requests;
use crate::gui::graph_ctx::input_ctx::InputCtx;
use crate::gui::pane::graph_ui::node_ui::const_editor::{ConstEditor, Field};
use crate::gui::path_type::PathType;

/// What an input's value cell holds, read once from its [`InputCtx`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum InputField<'a> {
    /// A constant, which its editor edits.
    Const(&'a ConstValue),
    /// Nothing, where a constant could sit: the empty field.
    Empty,
    /// A wire from this output feeds it.
    Wired(OutputPort),
    /// It takes a wire alone and has none.
    Unwired,
    /// A constant kept as a ghost: shown dimmed, edited by nothing.
    KeptConst(&'a ConstValue),
    /// A wire kept as a ghost.
    KeptWire,
}

impl<'a> InputField<'a> {
    /// What `input`'s cell holds. A ghost answers only where the input holds nothing, as a bind
    /// replaces it and a double-click clears it.
    pub(crate) fn of(input: InputCtx<'a>) -> Self {
        if input.binding().is_none()
            && let Some(ghost) = input.ghost()
        {
            return match &ghost.source {
                GhostSource::Const(value) => Self::KeptConst(value),
                GhostSource::Wire { .. } => Self::KeptWire,
            };
        }
        match input.binding() {
            Some(Binding::Const(value)) => Self::Const(value),
            Some(Binding::Wire(source)) => Self::Wired(*source),
            None if input.takes_const() => Self::Empty,
            None => Self::Unwired,
        }
    }
}

/// Record the editor of `input`'s constant, `value`, or its empty field, as `field` names it, and
/// raise what a commit means: a `SetInput` of the new constant, or, for a path's pick button, the
/// dialog only the shell can run.
pub(crate) fn record(
    ui: &mut Ui,
    out: &mut Requests,
    input: InputCtx<'_>,
    value: Option<&ConstValue>,
    field: Field<'_>,
) {
    let Some(value) = value else {
        field.empty(ui);
        return;
    };
    let port = input.port();
    let graph = input.graph();
    let data_type = input.data_type();
    // A path editor draws a pick button; a click on it raises a blocking file dialog, which only
    // `App` can run. Whether a click means "pick a path" is a question about the port's type,
    // which decided to draw the button, so it is answered here.
    if ui.response_for(field.id).clicked()
        && matches!(value, ConstValue::FsPath(_) | ConstValue::FsPaths(_))
        && let Some(path_type) = PathType::of(data_type)
    {
        out.push_app(AppCommand::Edit(EditCommand::PickInputPath(PathPick {
            graph,
            port,
            path_type,
        })));
    }
    let edited = ConstEditor {
        field,
        library: input.library(),
        value,
        data_type,
        domain: input.domain(),
    }
    .show(ui);
    if let Some(new_value) = edited {
        out.push_graph(
            graph,
            GraphIntent::set_input(port, Binding::Const(new_value)),
        );
    }
}

#[cfg(test)]
mod tests;
