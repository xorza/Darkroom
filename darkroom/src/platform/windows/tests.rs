use super::*;

#[test]
fn config_dir_sits_under_appdata() {
    assert_eq!(
        resolve_config_dir(Some(r"C:\Users\u\AppData\Roaming".into())),
        Some(PathBuf::from(r"C:\Users\u\AppData\Roaming\Darkroom"))
    );
}

#[test]
fn config_dir_is_none_without_a_usable_appdata() {
    for appdata in [
        None,
        Some(OsString::from("relative")),
        Some(OsString::new()),
    ] {
        assert_eq!(
            resolve_config_dir(appdata.clone()),
            None,
            "{appdata:?} is not a usable %APPDATA%"
        );
    }
}
