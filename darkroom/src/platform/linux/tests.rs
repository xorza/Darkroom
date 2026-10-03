use super::*;

#[test]
fn config_dir_prefers_an_absolute_xdg_config_home() {
    assert_eq!(
        resolve_config_dir(Some("/xdg".into()), Some("/home/u".into())),
        Some(PathBuf::from("/xdg/darkroom")),
        "an absolute XDG_CONFIG_HOME wins over HOME"
    );
}

#[test]
fn config_dir_falls_back_to_home_config() {
    // Unset, and — separately — set but relative, which the spec says to
    // ignore rather than resolve against the working directory.
    for xdg in [
        None,
        Some(OsString::from("relative/xdg")),
        Some(OsString::new()),
    ] {
        assert_eq!(
            resolve_config_dir(xdg.clone(), Some("/home/u".into())),
            Some(PathBuf::from("/home/u/.config/darkroom")),
            "{xdg:?} is not a usable XDG_CONFIG_HOME"
        );
    }
}

#[test]
fn config_dir_is_none_without_a_usable_home() {
    for home in [
        None,
        Some(OsString::from("relative")),
        Some(OsString::new()),
    ] {
        assert_eq!(
            resolve_config_dir(None, home.clone()),
            None,
            "{home:?} is not a usable HOME"
        );
    }
}
