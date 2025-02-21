using System.Diagnostics;
using System.Runtime.InteropServices;
using CoreInterop.Utils;
using YamlDotNet.Serialization;
using YamlDotNet.Serialization.NamingConventions;

namespace CoreInterop;

public unsafe partial class ScenariumCore : IDisposable {
    private static readonly Lock LOCK = new();
    private static int _ref_count = 0;

    [LibraryImport("kernel32.dll", EntryPoint = "LoadLibraryW", SetLastError = true,
        StringMarshalling = StringMarshalling.Utf16)]
    private static partial IntPtr LoadLibrary(string libname);

    [LibraryImport("kernel32.dll", EntryPoint = "FreeLibrary")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool FreeLibrary(IntPtr h_module);

    internal const string DLL_NAME = "core_interop.dll";

    [LibraryImport(DLL_NAME)]
    [UnmanagedCallConv(CallConvs = [typeof(System.Runtime.CompilerServices.CallConvCdecl)])]
    private static partial void* create_context();

    [LibraryImport(DLL_NAME)]
    [UnmanagedCallConv(CallConvs = [typeof(System.Runtime.CompilerServices.CallConvCdecl)])]
    private static partial void destroy_context(void* ctx);

    [LibraryImport(DLL_NAME)]
    [UnmanagedCallConv(CallConvs = [typeof(System.Runtime.CompilerServices.CallConvCdecl)])]
    private static partial FfiBuf get_graph(void* ctx);


    [LibraryImport(DLL_NAME)]
    [UnmanagedCallConv(CallConvs = [typeof(System.Runtime.CompilerServices.CallConvCdecl)])]
    private static partial FfiBuf get_func_lib(void* ctx);


    private static IntPtr _core_interop_handle = IntPtr.Zero;

    private static void LoadDll() {
        lock (LOCK) {
            if (_core_interop_handle != IntPtr.Zero) return;

            var sw = Stopwatch.StartNew();

            var full_dll_path = Path.GetFullPath(DLL_NAME);

            _core_interop_handle = LoadLibrary(full_dll_path);
            if (_core_interop_handle == IntPtr.Zero) {
                int error_code = Marshal.GetLastWin32Error();
                throw new Exception($"Failed to load library (ErrorCode: {error_code})");
            }

            _ref_count++;

            var elapsed = sw.ElapsedMilliseconds;
        }
    }

    private static void UnloadDll() {
        lock (LOCK) {
            if (_core_interop_handle == IntPtr.Zero) return;
            if (_ref_count == 0) return;

            FreeLibrary(_core_interop_handle);
            _core_interop_handle = IntPtr.Zero;
            _ref_count--;
        }
    }


    private void* _ctx = null;

    public ScenariumCore() {
        LoadDll();
        _ctx = create_context();
    }

    ~ScenariumCore() {
        ReleaseUnmanagedResources();
    }

    private readonly IDeserializer _deserializer = new DeserializerBuilder()
        .WithTagMapping("!Float", typeof(Value))
        .WithTagMapping("!Int", typeof(Value))
        .WithTagMapping("!Output", typeof(OutputBinding))
        .WithTagMapping("!Const", typeof(ConstBinding))
        .WithTypeConverter(new UuidConverter())
        .WithTypeConverter(new DataTypeConverter())
        .WithTypeConverter(new ValueConverter())
        .Build();


    public Graph GetGraph() {
        using var buf = get_graph(_ctx);
        var yaml = buf.ToString();

        return _deserializer.Deserialize<Graph>(new StringReader(yaml));
    }

    public FuncLib GetFuncLib() {
        using var buf = get_func_lib(_ctx);
        var yaml = buf.ToString();

        var funcs = _deserializer.Deserialize<List<Func>>(new StringReader(yaml));
        return new FuncLib() {
            Funcs = funcs
        };
    }

    private void ReleaseUnmanagedResources() {
        if (_ctx != null)
            destroy_context(_ctx);
        _ctx = null;
        UnloadDll();
    }

    public void Dispose() {
        ReleaseUnmanagedResources();
        GC.SuppressFinalize(this);
    }
}