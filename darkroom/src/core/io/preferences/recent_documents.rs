//! [`RecentDocuments`]: the documents File lists to open again.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};

/// The documents last opened or saved, newest first, each once, at most [`Self::CAP`].
///
/// Beside [`Preferences::document_path`](crate::core::io::preferences::Preferences), not in
/// its place: the document open at the last quit is `None` after File ▸ New while this list
/// still names files, so neither derives from the other.
///
/// Persisted as its paths. A file that repeats one or holds more than the cap loads as the
/// first of each, up to the cap, so a hand-edited file opens and the type holds its bound.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(crate) struct RecentDocuments {
    paths: Vec<PathBuf>,
}

impl RecentDocuments {
    /// One menu section a glance takes in.
    pub(crate) const CAP: usize = 10;

    /// Put `path` first, where it was listed or not, and drop what falls past the cap.
    pub(crate) fn note(&mut self, path: &Path) {
        self.forget(path);
        self.paths.insert(0, path.to_path_buf());
        self.paths.truncate(Self::CAP);
    }

    /// Take out `path`, one that no longer opens; nothing for a path not listed.
    pub(crate) fn forget(&mut self, path: &Path) {
        self.paths.retain(|listed| listed != path);
    }

    /// The paths, newest first.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &Path> {
        self.paths.iter().map(PathBuf::as_path)
    }
}

impl<'de> Deserialize<'de> for RecentDocuments {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let listed = Vec::<PathBuf>::deserialize(deserializer)?;
        let mut paths: Vec<PathBuf> = Vec::with_capacity(listed.len().min(Self::CAP));
        for path in listed {
            if paths.len() == Self::CAP {
                break;
            }
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        Ok(Self { paths })
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use common::SerdeFormat;

    use crate::core::io::preferences::recent_documents::RecentDocuments;

    fn listed(recent: &RecentDocuments) -> Vec<&str> {
        recent.iter().map(|path| path.to_str().unwrap()).collect()
    }

    /// A noted path goes first; one already listed moves there and is listed once; the eleventh
    /// falls off the end; a forgotten one goes, and forgetting a path not listed changes nothing.
    #[test]
    fn note_puts_a_path_first_once_under_the_cap() {
        let mut recent = RecentDocuments::default();
        for name in ["a", "b", "c"] {
            recent.note(Path::new(name));
        }
        assert_eq!(listed(&recent), ["c", "b", "a"]);
        recent.note(Path::new("a"));
        assert_eq!(listed(&recent), ["a", "c", "b"]);

        // 3 listed, 8 more: 11 noted in all, the cap keeps the newest 10, so "b" goes.
        for idx in 0..8 {
            recent.note(Path::new(&format!("n{idx}")));
        }
        assert_eq!(recent.iter().count(), RecentDocuments::CAP);
        assert_eq!(
            listed(&recent),
            ["n7", "n6", "n5", "n4", "n3", "n2", "n1", "n0", "a", "c"]
        );

        recent.forget(Path::new("n3"));
        recent.forget(Path::new("never listed"));
        assert_eq!(
            listed(&recent),
            ["n7", "n6", "n5", "n4", "n2", "n1", "n0", "a", "c"]
        );
    }

    /// The list round-trips as its paths, and a file of twelve with a repeat loads as the first
    /// ten distinct ones.
    #[test]
    fn a_decode_holds_the_cap_and_drops_repeats() {
        let mut recent = RecentDocuments::default();
        recent.note(Path::new("x"));
        recent.note(Path::new("y"));
        let bytes = SerdeFormat::Ron.serialize(&recent).unwrap();
        assert_eq!(
            str::from_utf8(&bytes).unwrap(),
            "[\n    \"y\",\n    \"x\",\n]"
        );
        let back: RecentDocuments = SerdeFormat::Ron.deserialize(&bytes).unwrap();
        assert_eq!(back, recent);

        // p0, p0 again, then p1..=p10: twelve entries, eleven distinct.
        let mut file: Vec<PathBuf> = vec![PathBuf::from("p0"), PathBuf::from("p0")];
        file.extend((1..=10).map(|idx| PathBuf::from(format!("p{idx}"))));
        assert_eq!(file.len(), 12);
        let bytes = SerdeFormat::Ron.serialize(&file).unwrap();
        let loaded: RecentDocuments = SerdeFormat::Ron.deserialize(&bytes).unwrap();
        assert_eq!(
            listed(&loaded),
            ["p0", "p1", "p2", "p3", "p4", "p5", "p6", "p7", "p8", "p9"]
        );
    }
}
