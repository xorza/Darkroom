use std::env;
use std::path::Path;

fn main() {
    let proto_file ="proto/graph.proto";
    let out_dir = env::var("OUT_DIR").unwrap();

    prost_build::Config::new()
        .out_dir(&out_dir)
        .compile_protos(&[proto_file], &[Path::new(".")])
        .expect("Failed to compile protobuf");
}
