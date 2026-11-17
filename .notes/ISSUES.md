# Open issues

- A ghost subscription whose trigger the node shows no glyph for — an `Outputs` node's trigger after its interface event is gone, or a trigger id other than `TriggerPort::NODE` on a node with its own — has no glyph at its trigger end, so `CanvasGeometry::trigger_center` finds none and its ghost event wire is not drawn.
