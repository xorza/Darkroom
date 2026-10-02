use crate::file_format::{FileExtensionError, SerdeFormat};

/// The extension picks the format, in any case and under any directory; none, or one no format
/// claims, is refused with the reason.
#[test]
fn from_file_name_maps_the_extension() {
    #[derive(Debug)]
    enum Expected {
        Format(SerdeFormat),
        Missing,
        Unsupported,
    }
    let cases = [
        ("a.ron", Expected::Format(SerdeFormat::Ron)),
        ("a.bin", Expected::Format(SerdeFormat::Bitcode)),
        ("a.lz4", Expected::Format(SerdeFormat::Lz4)),
        ("a.RON", Expected::Format(SerdeFormat::Ron)),
        ("a.Lz4", Expected::Format(SerdeFormat::Lz4)),
        ("/some/path/config.ron", Expected::Format(SerdeFormat::Ron)),
        ("path/to/file.bin", Expected::Format(SerdeFormat::Bitcode)),
        ("no_extension", Expected::Missing),
        ("", Expected::Missing),
        ("file.xyz", Expected::Unsupported),
    ];
    for (name, expected) in cases {
        let got = SerdeFormat::from_file_name(name);
        let matched = match (&got, &expected) {
            (Ok(format), Expected::Format(want)) => format == want,
            (Err(FileExtensionError::MissingFileExtension), Expected::Missing) => true,
            (Err(FileExtensionError::UnsupportedFileExtension(file)), Expected::Unsupported) => {
                file == name
            }
            _ => false,
        };
        assert!(matched, "{name:?}: got {got:?}, expected {expected:?}");
    }
}
