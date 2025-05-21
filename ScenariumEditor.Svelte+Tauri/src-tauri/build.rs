fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest()),
    )
    .expect("failed to run tauri-build");

    #[cfg(windows)]
    {
        // workaround needed to prevent STATUS_ENTRYPOINT_NOT_FOUND error in tests
        // see https://github.com/tauri-apps/tauri/pull/4383#issuecomment-1212221864
        let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
        let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
        let is_tauri_workspace =
            std::env::var("__TAURI_WORKSPACE__").map_or(false, |v| v == "true");

        if is_tauri_workspace && target_os == "windows" && target_env == "msvc" {
            static WINDOWS_MANIFEST_FILE: &str = "windows-app-manifest.xml";

            let manifest = std::env::current_dir().unwrap().join(WINDOWS_MANIFEST_FILE);

            println!("cargo:rerun-if-changed={}", manifest.display());
            println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
            println!(
                "cargo:rustc-link-arg=/MANIFESTINPUT:{}",
                manifest.to_str().unwrap()
            );
            println!("cargo:rustc-link-arg=/WX");
        }
    }
}
