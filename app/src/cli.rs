//! [`Cli`]: the command line, and the [`Launch`] it asks for. `darkroom/AGENTS.md`, "The editor
//! harness", "Flags on one binary".

use std::path::PathBuf;
use std::str::FromStr;

use clap::Parser;
use glam::UVec2;

/// The largest window side a session takes, in physical pixels: wgpu's default
/// `max_texture_dimension_2d`, which desktop adapters reach. palantir asks the adapter for its
/// own limit, so an adapter below it panics on a larger window.
const MAX_WINDOW_SIDE: u32 = 8192;
/// The smallest scale a session takes: at a quarter, a 12 px label is 3 pixels high.
const MIN_SCALE: f32 = 0.25;

/// darkroom — node-graph editor.
#[derive(Debug, Parser)]
#[command(version, about = "darkroom node-graph editor")]
pub(crate) struct Cli {
    /// Document to open, in place of the remembered one. Saving it is what makes the next bare
    /// launch reopen it.
    #[arg(value_name = "FILE")]
    document: Option<PathBuf>,
    /// Serve the editor to an agent as an MCP server on stdin and stdout.
    #[arg(long)]
    mcp: bool,
    /// With `--mcp`, no window: the editor renders offscreen, on a GPU adapter or a software one.
    #[arg(long, requires = "mcp")]
    headless: bool,
    /// The headless window's physical size, as `1600x1000`.
    #[arg(long, requires = "headless", default_value = "1600x1000")]
    size: WindowSize,
    /// The headless window's physical pixels per logical pixel, 0.25 or more.
    #[arg(long, requires = "headless", default_value = "1")]
    scale: WindowScale,
}

/// What a launch runs.
#[derive(Debug, PartialEq)]
pub(crate) enum Launch {
    /// The editor's window, on `document` or the remembered one.
    Window { document: Option<PathBuf> },
    /// The window, and an MCP server that drives it.
    McpWindow { document: Option<PathBuf> },
    /// An MCP server that drives the editor in an offscreen window of `size` physical pixels at
    /// `scale`, on `document` or a new one.
    McpHeadless {
        document: Option<PathBuf>,
        size: UVec2,
        scale: f32,
    },
}

/// A `WxH` size in pixels, each side from 1 to [`MAX_WINDOW_SIDE`]: a window no device can make
/// is refused with the usage error.
#[derive(Clone, Copy, Debug)]
struct WindowSize(UVec2);

/// Physical pixels per logical pixel, finite and at least [`MIN_SCALE`], so a label stays
/// readable.
#[derive(Clone, Copy, Debug)]
struct WindowScale(f32);

impl Cli {
    pub(crate) fn launch(self) -> Launch {
        let Self {
            document,
            mcp,
            headless,
            size,
            scale,
        } = self;
        match (mcp, headless) {
            (false, _) => Launch::Window { document },
            (true, false) => Launch::McpWindow { document },
            (true, true) => Launch::McpHeadless {
                document,
                size: size.0,
                scale: scale.0,
            },
        }
    }
}

impl FromStr for WindowSize {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bad = || format!("'{text}' is not a size; write it as 1600x1000");
        let (w, h) = text.split_once('x').ok_or_else(bad)?;
        let size = UVec2::new(w.parse().map_err(|_| bad())?, h.parse().map_err(|_| bad())?);
        if size.min_element() == 0 || size.max_element() > MAX_WINDOW_SIDE {
            return Err(format!(
                "'{text}' is no window; give each side from 1 to {MAX_WINDOW_SIDE} pixels"
            ));
        }
        Ok(Self(size))
    }
}

impl FromStr for WindowScale {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.parse::<f32>() {
            Ok(scale) if scale.is_finite() && scale >= MIN_SCALE => Ok(Self(scale)),
            _ => Err(format!(
                "'{text}' is no scale; give the physical pixels per logical pixel, {MIN_SCALE} \
                 or more"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;
    use clap::error::ErrorKind;
    use glam::UVec2;

    use crate::cli::{Cli, Launch, WindowScale, WindowSize};

    fn launch(args: &[&str]) -> Result<Launch, ErrorKind> {
        Cli::try_parse_from(["darkroom"].iter().chain(args))
            .map(Cli::launch)
            .map_err(|error| error.kind())
    }

    /// Each launch the flags name, the headless window's defaults and overrides included; a
    /// headless flag without `--mcp`, a size or scale without `--headless`, an unknown flag and a
    /// second document are refused.
    #[test]
    fn the_flags_pick_the_launch() {
        let scene = || Some(PathBuf::from("graphs/scene.darkroom"));
        assert_eq!(launch(&[]), Ok(Launch::Window { document: None }));
        assert_eq!(
            launch(&["graphs/scene.darkroom"]),
            Ok(Launch::Window { document: scene() })
        );
        assert_eq!(
            launch(&["--mcp", "graphs/scene.darkroom"]),
            Ok(Launch::McpWindow { document: scene() })
        );
        assert_eq!(
            launch(&["--mcp", "--headless"]),
            Ok(Launch::McpHeadless {
                document: None,
                size: UVec2::new(1600, 1000),
                scale: 1.0,
            })
        );
        assert_eq!(
            launch(&[
                "--mcp",
                "--headless",
                "--size",
                "800x600",
                "--scale",
                "2",
                "graphs/scene.darkroom"
            ]),
            Ok(Launch::McpHeadless {
                document: scene(),
                size: UVec2::new(800, 600),
                scale: 2.0,
            })
        );

        for refused in [
            &["--headless"][..],
            &["--mcp", "--size", "800x600"],
            &["--mcp", "--scale", "2"],
            &["--size", "800x600"],
        ] {
            assert_eq!(
                launch(refused),
                Err(ErrorKind::MissingRequiredArgument),
                "{refused:?}"
            );
        }
        assert_eq!(launch(&["--nope"]), Err(ErrorKind::UnknownArgument));
        assert_eq!(
            launch(&["one.darkroom", "two.darkroom"]),
            Err(ErrorKind::UnknownArgument),
            "only one document opens per launch"
        );
    }

    /// A size is `WxH` in whole pixels, each side from 1 to 8192, the edges taken; anything else
    /// is refused with the form to write. A scale is a number from a quarter, the edge taken.
    #[test]
    fn a_window_size_is_width_x_height_and_a_scale_reads_at_a_quarter() {
        for (text, size) in [("1600x1000", [1600, 1000]), ("1x8192", [1, 8192])] {
            assert_eq!(text.parse::<WindowSize>().unwrap().0, UVec2::from(size));
        }
        for bad in ["1600", "1600x", "x1000", "16o0x1000", "-1x1"] {
            assert_eq!(
                bad.parse::<WindowSize>().unwrap_err(),
                format!("'{bad}' is not a size; write it as 1600x1000")
            );
        }
        for none in ["0x600", "8193x600"] {
            assert_eq!(
                none.parse::<WindowSize>().unwrap_err(),
                format!("'{none}' is no window; give each side from 1 to 8192 pixels")
            );
        }
        assert_eq!("0.25".parse::<WindowScale>().unwrap().0, 0.25);
        for bad in ["0.24", "NaN", "inf", "-2", "two"] {
            assert_eq!(
                bad.parse::<WindowScale>().unwrap_err(),
                format!(
                    "'{bad}' is no scale; give the physical pixels per logical pixel, 0.25 or more"
                )
            );
        }
        assert_eq!(
            launch(&["--mcp", "--headless", "--size", "0x600"]),
            Err(ErrorKind::ValueValidation)
        );
    }
}
