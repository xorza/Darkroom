# Wire ties

A tie gathers wires into a ribbon on the graph canvas, as a cable tie gathers cables. It is
layout only: the graph does not change, and the run does not read it. Mockups and a playground:
<https://claude.ai/artifact/VZPoqKGdJTEuPSsAE6hcxK>. The plan is `WIRE_TIES_PLAN.md`.

## Decisions

- **A tie is view data.** Ties and routes live in `GraphView` beside the node placements: they
  are saved, undoable, and ignored by compile, plan and run. Refused: a pass-through node, as
  Substance Designer's Cable Tie node does, which changes the graph for a layout need; scenarium
  has no design for reroute nodes ([Cable Tie](https://jclark.gumroad.com/l/EZAgl)).
- **A route is stored, a strand's slot is derived.** Each tied wire stores its ordered route of
  tie ids. Each frame, a strand's slot comes from the barycenter of its two neighbouring points
  along the band axis, which is the crossing-reduction heuristic of layered graph drawing
  (Sugiyama, Tagawa, Toda 1981). At creation this is the order the line crossed the wires. Refused:
  a stored order, as Zip Ties stores one, because strands can cross inside the band after nodes
  move.
- **The tie line is Shift+RMB-drag, and Ctrl+Shift+LMB-drag on a trackpad.** Blender draws a
  line across links with Shift+RMB to add reroutes, beside its Ctrl+RMB link cut. darkroom's
  breaker is RMB and Ctrl+LMB, so the tie line is the breaker's chord plus Shift. Refused: hold
  Z, as ComfyUI Zip Ties does, because palantir has no key-release event; Alt+LMB, because
  Alt-drag moves windows on some Linux desktops and Blender uses it to detach links
  ([Blender Reroute](https://docs.blender.org/manual/en/latest/interface/controls/nodes/types/layout/reroute.html),
  [KWin MR 12](https://invent.kde.org/plasma/kwin/-/merge_requests/12)).
- **The line only ties.** It ties every crossed wire into one new tie at the centre of the
  crossings. A line that crosses a tie adds the other crossed wires to that tie. Delete or the
  tie's menu removes a tie. Refused: a line that removes the tie it crosses, as both ComfyUI
  extensions do, which gives one gesture two opposite results and no way to add a wire.
- **A tie holds wires of both families.** Data and event wires mix in one tie, as the breaker
  cuts both. A route is keyed by `WireKey`, `Data(InputPort)` or `Event(Subscription)`, the emitter's
  port and the subscriber.
  Refused: data wires only, and one tie for each family.
- **A wire is both of its ends.** Rewiring either end, disconnecting or removing a node drops
  the wire's route in the same undo step, and a tie with no wires goes with it. Refused: a route
  keyed by the input alone, which puts a strand from an unrelated output into a bundle. Zip Ties
  follows the same rule.
- **A tie is an item.** `GraphView::selected` holds `ItemId`, `Node(NodeId)` or `Tie(TieId)`.
  Band select, click, Shift-click, Delete, Esc and the group drag treat a tie as they treat a
  node, as Blender and Unreal treat reroute points. Refused: a second selection set kept in
  step by hand, and ties that cannot be selected.
- **A node set takes the ties inside it.** A drag or a duplicate of a node set also takes each
  tie whose every wire has both end nodes in the set, plus the selected ties for a drag. One rule
  for move and duplicate, as Zip Ties uses for its moves and copies. Refused: Blender's rule,
  where only selected items move and a ribbon stretches when its nodes are clicked and dragged.
- **The band turns itself, and a knob pins it.** An unpinned band stands square to the mean
  direction of its strands. Dragging the knob of a selected tie pins the angle: free, or in 15°
  steps with Shift, as design tools snap. A double-click on the knob frees it
  ([comfyui-cabletie](https://github.com/nathancoooper/comfyui-cabletie)).
- **One wire leaves through the tie's menu.** RMB-click on a tie opens the anchored menu:
  Release ▸ one entry per wire, Pin or Free angle, Remove tie. Refused: dragging a strand out of
  the band, a target 2.5 units wide that would need wire hit-testing, which the canvas avoids.
- **A tied wire is one cubic per piece.** The pieces run from port to tie, tie to tie, and tie to
  port, with the tangent along the strands at each tie, so the curve stays G1. An untied wire
  keeps `Wire::data` and `Wire::event`.
- **The band is neutral and paints over wires, under nodes.** The band takes the muted ink and
  lifts on hover and selection, so the wire colours, which are the types, stay clear. It belongs
  to the plumbing, so a node body covers it as it covers wires. Refused: a colour for each tie.
- **No motion of its own.** A band follows the pointer and the nodes directly. Refused: the spring
  motion of comfyui-cabletie, which lags the pointer and repaints the canvas until it settles.
