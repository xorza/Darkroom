use std::process::Command;

fn main() {
    let fbs_file = "fbs/interop.fbs";

    Command::new("flatc.exe")
        .args(["--rust", "--gen-object-api", "-o", "src/gen", fbs_file])
        .status()
        .expect("Failed to generate Rust bindings");

    // println!("cargo:rerun-if-changed={}", fbs_file);
}
