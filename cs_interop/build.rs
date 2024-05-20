fn main() {
    csbindgen::Builder::default()
        .input_extern_file("src/lib.rs")
        .csharp_dll_name("core_interop")
        .csharp_class_name("CoreNative")
        .csharp_namespace("CoreInterop")
        .generate_csharp_file("../ScenariumEditor.NET/CoreInterop/CoreNative.g.cs")

        .unwrap();
}