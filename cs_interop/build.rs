fn main() {
    csbindgen::Builder::default()
        .input_extern_file("src/lib.rs")
        .csharp_dll_name("core_interop")
        .generate_csharp_file("../ScenariumEditor.NET/CoreInterop/NativeMethods.g.cs")
        .unwrap();
}