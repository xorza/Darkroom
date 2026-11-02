//! [`ErrorChain`]: an error and every cause under it, as one line of text.

use std::error::Error;
use std::fmt;

/// An error and each cause [`Error::source`] reaches under it, joined by `": "`, as anyhow's
/// `{:#}` and std's unstable `std::error::Report` print a chain. Every error of the workspace
/// names its cause in `source` alone, so a report that prints the error by itself leaves out what
/// the layers under it said.
///
/// A foreign error can print its cause and return it as its source too, as `tiff::TiffError::IoError`
/// does. A layer whose text ends with its cause's text, after a space, a colon or nothing, drops
/// that end, and a layer left with no text of its own drops out, as snafu's `CleanedErrorText`
/// cleans a chain, so each message prints once. The boundary keeps a word that only ends like its
/// cause, as "reload" over "load", whole.
/// That comparison formats each layer of a chain into a `String`; an error with no source prints
/// straight through, so a reader that displays a leaf every frame allocates nothing.
#[derive(Clone, Copy, Debug)]
pub struct ErrorChain<'a>(&'a dyn Error);

impl<'a> ErrorChain<'a> {
    pub const fn new(error: &'a dyn Error) -> Self {
        Self(error)
    }
}

impl fmt::Display for ErrorChain<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut next = self.0.source();
        if next.is_none() {
            return write!(f, "{}", self.0);
        }
        let mut text = self.0.to_string();
        let mut first = true;
        loop {
            let next_text = next.map(ToString::to_string);
            let own = next_text
                .as_deref()
                .and_then(|cause| text.strip_suffix(cause))
                .filter(|rest| rest.is_empty() || rest.ends_with([' ', ':']))
                .map_or(text.as_str(), |rest| {
                    let rest = rest.trim_end();
                    rest.strip_suffix(':').unwrap_or(rest).trim_end()
                });
            if !own.is_empty() {
                if !first {
                    f.write_str(": ")?;
                }
                f.write_str(own)?;
                first = false;
            }
            let (Some(cause), Some(cause_text)) = (next, next_text) else {
                return Ok(());
            };
            text = cause_text;
            next = cause.source();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use crate::ErrorChain;

    #[derive(Debug, thiserror::Error)]
    #[error("the {layer} failed")]
    struct Layer {
        layer: &'static str,
        #[source]
        source: Option<Box<Self>>,
    }

    #[derive(Debug, thiserror::Error)]
    #[error("could not read the file")]
    struct Read(#[source] io::Error);

    /// A foreign error that breaks the rule: it prints its cause and returns it as its source.
    #[derive(Debug, thiserror::Error)]
    #[error("{1}{0}")]
    struct Repeats(#[source] io::Error, &'static str);

    #[derive(Debug, thiserror::Error)]
    #[error("the decode failed")]
    struct Decode(#[source] Repeats);

    #[test]
    fn a_leaf_prints_alone() {
        let leaf = Layer {
            layer: "codec",
            source: None,
        };
        assert_eq!(ErrorChain::new(&leaf).to_string(), "the codec failed");
    }

    #[test]
    fn each_cause_follows_the_error_in_order() {
        let chain = Layer {
            layer: "load",
            source: Some(Box::new(Layer {
                layer: "decode",
                source: Some(Box::new(Layer {
                    layer: "codec",
                    source: None,
                })),
            })),
        };
        assert_eq!(
            ErrorChain::new(&chain).to_string(),
            "the load failed: the decode failed: the codec failed"
        );
        let read = Read(io::Error::other("disk full"));
        assert_eq!(
            ErrorChain::new(&read).to_string(),
            "could not read the file: disk full"
        );
    }

    /// The text a layer repeats from its cause prints once: the separator before it goes with it,
    /// and a layer that is only its cause's text drops out. Text that only ends like the cause,
    /// inside a word, is the layer's own.
    #[test]
    fn a_cause_a_layer_repeats_prints_once() {
        for (prefix, chain) in [
            ("", "the decode failed: disk full"),
            ("tile 3: ", "the decode failed: tile 3: disk full"),
            ("tile 3 ", "the decode failed: tile 3: disk full"),
            ("re", "the decode failed: redisk full: disk full"),
        ] {
            let decode = Decode(Repeats(io::Error::other("disk full"), prefix));
            assert_eq!(ErrorChain::new(&decode).to_string(), chain, "{prefix:?}");
        }
    }
}
