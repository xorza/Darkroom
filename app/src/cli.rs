//! [`parse`]: the command line, and the [`Command`] it asks for. `darkroom/AGENTS.md`, "The
//! editor harness", "Flags on one binary".
//!
//! Parsed by hand: one optional document and five flags, whose rules are checked in one place.
//! Refused: `clap`, five crates for that. A usage error exits with 2, as clap's and the
//! POSIX utilities' do.

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use darkroom::RunConfig;
use glam::UVec2;
use palantir::{DebugOverlayConfig, SystemScale};

/// The largest window side a session takes, in physical pixels: wgpu's default
/// `max_texture_dimension_2d`, which desktop adapters reach. An adapter below it answers a larger
/// window with `DriverError::WindowTooLarge`.
const MAX_WINDOW_SIDE: u32 = 8192;
/// The smallest scale a session takes: at a quarter, a 12 px label is 3 pixels high.
const MIN_SCALE: f32 = 0.25;

/// What `--help` prints.
pub(crate) const HELP: &str = "\
darkroom node-graph editor

Usage: darkroom [OPTIONS] [FILE]

Arguments:
  [FILE]  Document to open, in place of the remembered one. Saving it is what makes the next
          bare launch reopen it

Options:
      --mcp            Serve the editor to an agent as an MCP server on stdin and stdout. The
                       editor starts on the default preferences and saves none
      --headless       With --mcp, no window: the editor renders offscreen, on a GPU adapter or a
                       software one
      --debug-overlay  Paint each frame's damage and the frame stats over the window
      --size <WxH>     The headless window's physical size [default: 1600x1000]
      --scale <SCALE>  The headless window's physical pixels per logical pixel, 0.25 or more
                       [default: 1]
  -h, --help           Print help
  -V, --version        Print version
";

/// What the command line asks for.
#[derive(Debug, PartialEq)]
pub(crate) enum Command {
    Launch(Launch),
    /// Print [`HELP`].
    Help,
    /// Print the version.
    Version,
}

/// What a launch runs.
#[derive(Debug, PartialEq)]
pub(crate) enum Launch {
    /// The editor's window, on the config's document or the remembered one.
    Window(RunConfig),
    /// The window on the default preferences, which it saves nowhere, and an MCP server that
    /// drives it.
    McpWindow(RunConfig),
    /// An MCP server that drives the editor in an offscreen window of `size` physical pixels at
    /// `scale`, on `document` or a new one.
    McpHeadless {
        document: Option<PathBuf>,
        size: UVec2,
        scale: SystemScale,
    },
}

/// Why a command line asks for nothing the binary does.
#[derive(Debug, PartialEq)]
pub(crate) enum CliError {
    /// An argument that starts with `-` and is no flag.
    UnknownFlag(String),
    /// A flag given twice.
    Repeated(&'static str),
    /// A second document; one opens per launch.
    SecondDocument(PathBuf),
    /// A flag that takes a value, last on the line.
    MissingValue(&'static str),
    /// A flag that takes no value, given one after `=`.
    UnexpectedValue(&'static str),
    /// A flag's value its type refuses, with why.
    Value { flag: &'static str, why: String },
    /// A flag given without the one it needs.
    Requires {
        flag: &'static str,
        needs: &'static str,
    },
    /// Two flags that exclude each other.
    Conflicts {
        flag: &'static str,
        with: &'static str,
    },
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFlag(flag) => write!(f, "unexpected argument '{flag}'"),
            Self::Repeated(flag) => write!(f, "the argument '{flag}' was given more than once"),
            Self::SecondDocument(path) => write!(
                f,
                "unexpected argument '{}': one document opens per launch",
                path.display()
            ),
            Self::MissingValue(flag) => write!(f, "a value is required for '{flag}'"),
            Self::UnexpectedValue(flag) => write!(f, "'{flag}' takes no value"),
            Self::Value { flag, why } => write!(f, "invalid value for '{flag}': {why}"),
            Self::Requires { flag, needs } => write!(f, "'{flag}' needs '{needs}'"),
            Self::Conflicts { flag, with } => {
                write!(f, "'{flag}' cannot be used with '{with}'")
            }
        }
    }
}

/// A `WxH` size in pixels, each side from 1 to [`MAX_WINDOW_SIDE`]: a window no device can make
/// is refused with the usage error.
#[derive(Clone, Copy, Debug)]
struct WindowSize(UVec2);

/// Physical pixels per logical pixel, finite and at least [`MIN_SCALE`], so a label stays
/// readable.
#[derive(Clone, Copy, Debug)]
struct WindowScale(SystemScale);

/// The flags as a command line gives them, each at most once.
#[derive(Debug, Default)]
struct Flags {
    document: Option<PathBuf>,
    mcp: bool,
    headless: bool,
    debug_overlay: bool,
    size: Option<WindowSize>,
    scale: Option<WindowScale>,
}

/// The [`Command`] `args`, the command line past the program's name, asks for. A flag's value
/// follows it or an `=`; after `--`, every argument is the document.
pub(crate) fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, CliError> {
    let mut args = args.into_iter();
    let mut flags = Flags::default();
    let mut only_documents = false;
    while let Some(arg) = args.next() {
        let text = arg.to_str();
        if only_documents || !text.is_some_and(|text| text.starts_with('-')) {
            if flags.document.is_some() {
                return Err(CliError::SecondDocument(PathBuf::from(arg)));
            }
            flags.document = Some(PathBuf::from(arg));
            continue;
        }
        let text = text.expect("a flag is text");
        let (name, inline) = match text.split_once('=') {
            Some((name, value)) => (name, Some(value.to_owned())),
            None => (text, None),
        };
        let mut value = |flag: &'static str| match inline.clone() {
            Some(value) => Ok(value),
            None => args
                .next()
                .ok_or(CliError::MissingValue(flag))?
                .into_string()
                .map_err(|_| CliError::Value {
                    flag,
                    why: "the value is not text".to_owned(),
                }),
        };
        let bare = |flag: &'static str| match inline {
            Some(_) => Err(CliError::UnexpectedValue(flag)),
            None => Ok(()),
        };
        match name {
            "--" => {
                bare("--")?;
                only_documents = true;
            }
            "-h" | "--help" => {
                bare("--help")?;
                return Ok(Command::Help);
            }
            "-V" | "--version" => {
                bare("--version")?;
                return Ok(Command::Version);
            }
            "--mcp" => {
                bare("--mcp")?;
                set(&mut flags.mcp, "--mcp")?;
            }
            "--headless" => {
                bare("--headless")?;
                set(&mut flags.headless, "--headless")?;
            }
            "--debug-overlay" => {
                bare("--debug-overlay")?;
                set(&mut flags.debug_overlay, "--debug-overlay")?;
            }
            "--size" => {
                let size = parsed(&value("--size")?, "--size")?;
                once(&mut flags.size, size, "--size")?;
            }
            "--scale" => {
                let scale = parsed(&value("--scale")?, "--scale")?;
                once(&mut flags.scale, scale, "--scale")?;
            }
            _ => return Err(CliError::UnknownFlag(text.to_owned())),
        }
    }
    flags.launch().map(Command::Launch)
}

/// Turn a boolean flag on, refusing it the second time.
const fn set(flag: &mut bool, name: &'static str) -> Result<(), CliError> {
    if *flag {
        return Err(CliError::Repeated(name));
    }
    *flag = true;
    Ok(())
}

/// Hold a valued flag's value, refusing it the second time.
fn once<T>(slot: &mut Option<T>, value: T, name: &'static str) -> Result<(), CliError> {
    if slot.is_some() {
        return Err(CliError::Repeated(name));
    }
    *slot = Some(value);
    Ok(())
}

/// `text` as a flag's value of `T`.
fn parsed<T: FromStr<Err = String>>(text: &str, flag: &'static str) -> Result<T, CliError> {
    text.parse().map_err(|why| CliError::Value { flag, why })
}

impl Flags {
    /// The launch the flags name, once their rules hold: `--headless` needs `--mcp`, `--size`
    /// and `--scale` need `--headless`, and `--debug-overlay` paints over a window `--headless`
    /// has none of.
    fn launch(self) -> Result<Launch, CliError> {
        let Self {
            document,
            mcp,
            headless,
            debug_overlay,
            size,
            scale,
        } = self;
        if headless && !mcp {
            return Err(CliError::Requires {
                flag: "--headless",
                needs: "--mcp",
            });
        }
        for (given, flag) in [(size.is_some(), "--size"), (scale.is_some(), "--scale")] {
            if given && !headless {
                return Err(CliError::Requires {
                    flag,
                    needs: "--headless",
                });
            }
        }
        if headless && debug_overlay {
            return Err(CliError::Conflicts {
                flag: "--debug-overlay",
                with: "--headless",
            });
        }
        let window = |document| RunConfig {
            document,
            overlay: DebugOverlayConfig {
                damage_rect: debug_overlay,
                frame_stats: debug_overlay,
                ..DebugOverlayConfig::default()
            },
        };
        Ok(match (mcp, headless) {
            (false, _) => Launch::Window(window(document)),
            (true, false) => Launch::McpWindow(window(document)),
            (true, true) => Launch::McpHeadless {
                document,
                size: size.map_or(UVec2::new(1600, 1000), |size| size.0),
                scale: scale.map_or(SystemScale::ONE, |scale| scale.0),
            },
        })
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
        text.parse::<f32>()
            .ok()
            .filter(|scale| *scale >= MIN_SCALE)
            .and_then(SystemScale::new)
            .map(Self)
            .ok_or_else(|| {
                format!(
                    "'{text}' is no scale; give the physical pixels per logical pixel, \
                     {MIN_SCALE} or more"
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use std::ffi::OsString;

    use darkroom::RunConfig;
    use glam::UVec2;
    use palantir::{DebugOverlayConfig, SystemScale};

    use crate::cli::{CliError, Command, Launch, WindowScale, WindowSize, parse};

    fn command(args: &[&str]) -> Result<Command, CliError> {
        parse(args.iter().map(OsString::from))
    }

    fn launch(args: &[&str]) -> Result<Launch, CliError> {
        command(args).map(|command| match command {
            Command::Launch(launch) => launch,
            other => panic!("{args:?} asked for {other:?}, no launch"),
        })
    }

    /// Each launch the flags name, the headless window's defaults and overrides included, a
    /// value after `=` as after a space, and a document after `--` that starts with a dash; help
    /// and the version answer whatever follows them; a headless flag without `--mcp`, a size or
    /// scale without `--headless`, an overlay on no window, a flag twice, a flag with no value, a
    /// flag of none given one, an unknown flag and a second document are refused.
    #[test]
    fn the_flags_pick_the_launch() {
        let scene = || Some(PathBuf::from("graphs/scene.darkroom"));
        let opened = |document| RunConfig {
            document,
            overlay: DebugOverlayConfig::default(),
        };
        let overlaid = |document| RunConfig {
            document,
            overlay: DebugOverlayConfig {
                damage_rect: true,
                frame_stats: true,
                dim_undamaged: false,
            },
        };
        assert_eq!(launch(&[]), Ok(Launch::Window(opened(None))));
        assert_eq!(
            launch(&["graphs/scene.darkroom"]),
            Ok(Launch::Window(opened(scene())))
        );
        assert_eq!(
            launch(&["--mcp", "graphs/scene.darkroom"]),
            Ok(Launch::McpWindow(opened(scene())))
        );
        assert_eq!(
            launch(&["--debug-overlay"]),
            Ok(Launch::Window(overlaid(None)))
        );
        assert_eq!(
            launch(&["--mcp", "--debug-overlay", "graphs/scene.darkroom"]),
            Ok(Launch::McpWindow(overlaid(scene())))
        );
        assert_eq!(
            launch(&["--mcp", "--headless", "--debug-overlay"]),
            Err(CliError::Conflicts {
                flag: "--debug-overlay",
                with: "--headless"
            }),
            "a headless session has no window to paint over"
        );
        assert_eq!(
            launch(&["--mcp", "--headless"]),
            Ok(Launch::McpHeadless {
                document: None,
                size: UVec2::new(1600, 1000),
                scale: SystemScale::ONE,
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
                scale: SystemScale::new(2.0).unwrap(),
            })
        );

        assert_eq!(
            launch(&[
                "--mcp",
                "--headless",
                "--size=800x600",
                "--",
                "-scene.darkroom"
            ]),
            Ok(Launch::McpHeadless {
                document: Some(PathBuf::from("-scene.darkroom")),
                size: UVec2::new(800, 600),
                scale: SystemScale::ONE,
            })
        );
        for (help, version) in [("--help", "--version"), ("-h", "-V")] {
            assert_eq!(command(&["--mcp", help, "--nope"]), Ok(Command::Help));
            assert_eq!(command(&[version]), Ok(Command::Version));
        }

        let requires = |flag, needs| Err(CliError::Requires { flag, needs });
        assert_eq!(launch(&["--headless"]), requires("--headless", "--mcp"));
        assert_eq!(
            launch(&["--mcp", "--size", "800x600"]),
            requires("--size", "--headless")
        );
        assert_eq!(
            launch(&["--mcp", "--scale", "2"]),
            requires("--scale", "--headless")
        );
        assert_eq!(
            launch(&["--size", "800x600"]),
            requires("--size", "--headless")
        );
        assert_eq!(
            launch(&["--mcp", "--mcp"]),
            Err(CliError::Repeated("--mcp"))
        );
        assert_eq!(
            launch(&["--mcp=yes"]),
            Err(CliError::UnexpectedValue("--mcp"))
        );
        assert_eq!(
            command(&["--help=all"]),
            Err(CliError::UnexpectedValue("--help"))
        );
        assert_eq!(
            launch(&["--mcp", "--headless", "--size", "1x1", "--size", "2x2"]),
            Err(CliError::Repeated("--size"))
        );
        assert_eq!(
            launch(&["--mcp", "--headless", "--scale"]),
            Err(CliError::MissingValue("--scale"))
        );
        assert_eq!(
            launch(&["--nope"]),
            Err(CliError::UnknownFlag("--nope".to_owned()))
        );
        assert_eq!(
            launch(&["one.darkroom", "two.darkroom"]),
            Err(CliError::SecondDocument(PathBuf::from("two.darkroom"))),
            "only one document opens per launch"
        );
    }

    /// Each refusal names what is wrong and with which flag.
    #[test]
    fn each_refusal_says_why() {
        for (error, message) in [
            (
                CliError::UnknownFlag("--nope".to_owned()),
                "unexpected argument '--nope'",
            ),
            (
                CliError::Repeated("--mcp"),
                "the argument '--mcp' was given more than once",
            ),
            (
                CliError::SecondDocument(PathBuf::from("b.darkroom")),
                "unexpected argument 'b.darkroom': one document opens per launch",
            ),
            (
                CliError::MissingValue("--size"),
                "a value is required for '--size'",
            ),
            (CliError::UnexpectedValue("--mcp"), "'--mcp' takes no value"),
            (
                CliError::Value {
                    flag: "--scale",
                    why: "too small".to_owned(),
                },
                "invalid value for '--scale': too small",
            ),
            (
                CliError::Requires {
                    flag: "--headless",
                    needs: "--mcp",
                },
                "'--headless' needs '--mcp'",
            ),
            (
                CliError::Conflicts {
                    flag: "--debug-overlay",
                    with: "--headless",
                },
                "'--debug-overlay' cannot be used with '--headless'",
            ),
        ] {
            assert_eq!(error.to_string(), message);
        }
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
        assert_eq!("0.25".parse::<WindowScale>().unwrap().0.get(), 0.25);
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
            Err(CliError::Value {
                flag: "--size",
                why: "'0x600' is no window; give each side from 1 to 8192 pixels".to_owned(),
            })
        );
    }
}
