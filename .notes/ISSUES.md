# Open issues

- `darkroom/src/gui/widgets/pill.rs`: the toolbar pills are documented as frosted, but their chrome is a plain fill of `surround_bg` at 0.7 alpha with no backdrop filter. Over a bright image, such as under the image viewer's toolbar, nothing bounds what shows through, so the contrast of a pill's labels and readouts falls with the image under it.
