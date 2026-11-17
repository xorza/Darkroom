# Open issues


- `Graph::admit_subscription` admits `Listener::Pin` on any node with a body, while the editor draws a subscription pin only on a sink (`NodeCtx::sink`) and a ghost pin only for ghost wiring; a live subscription to a non-sink func's pin (one whose func stopped being a sink in the library, say) has no glyph at its listener end.

- `gui::app::tests::the_input_stage_acts_once_over_a_frames_two_passes` fails on master at "the node followed": dragging Auto Stretch's footer by (30, 20) moves the node by about 2.23 times that in `sample/sample.darkroom`.
