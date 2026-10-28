# Open issues
- **A graph pane grows past the window to hold a node.** In `driver::tests::a_shot_is_the_frame_with_a_legend_of_what_it_shows` at 400 × 300, a Brightness / Contrast node with four inputs at (40, 119) reaches y 311, and the Graph pane's rect becomes (0, 41) to (400, 311), past the 300 px window, while the node reads 192 px tall against 193 px in a 400 × 340 window.
