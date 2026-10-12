// Release builds link as a Windows GUI binary, so double-clicking the .exe in
// Explorer opens the window alone instead of trailing a console behind it.
// Dev builds keep the console subsystem — that is where `init_tracing`'s
// stderr output and clap's `--help` land. Gated on the same `debug_assertions`
// signal as `common::is_debug`; `release-max` inherits `release`, so it has it
// off and gets the GUI subsystem.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![deny(unsafe_code)]

use std::path::PathBuf;
use std::process;

use clap::Parser;

/// darkroom — node-graph editor.
#[derive(Parser, Debug)]
#[command(version, about = "darkroom node-graph editor")]
struct Cli {
    /// Document to open, in place of the remembered one. Saving it is what
    /// makes the next bare launch reopen it.
    #[arg(value_name = "FILE")]
    document: Option<PathBuf>,
}

fn main() {
    darkroom::init_tracing();

    let Cli { document } = Cli::parse();
    if let Err(error) = darkroom::run(document) {
        tracing::error!("darkroom: {error}");
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;

    use crate::Cli;

    #[test]
    fn bare_invocation_parses() {
        let cli = Cli::try_parse_from(["darkroom"]).unwrap();
        assert_eq!(cli.document, None);
        assert!(
            Cli::try_parse_from(["darkroom", "--nope"]).is_err(),
            "an unknown flag is still rejected"
        );
    }

    #[test]
    fn a_document_argument_parses() {
        let cli = Cli::try_parse_from(["darkroom", "graphs/scene.darkroom"]).unwrap();
        assert_eq!(cli.document, Some(PathBuf::from("graphs/scene.darkroom")));
        assert!(
            Cli::try_parse_from(["darkroom", "one.darkroom", "two.darkroom"]).is_err(),
            "only one document opens per launch"
        );
    }
}
