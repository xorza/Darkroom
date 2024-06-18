using System.Diagnostics;
using System.Runtime.InteropServices;
using CoreInterop.Utils;
using YamlDotNet.Serialization;
using YamlDotNet.Serialization.NamingConventions;

namespace CoreInterop;

public unsafe class ScenariumCore {
    [DllImport("kernel32.dll", CharSet = CharSet.Auto, SetLastError = true)]
    private static extern IntPtr LoadLibrary(string libname);

    [DllImport("kernel32.dll", CharSet = CharSet.Auto)]
    private static extern bool FreeLibrary(IntPtr h_module);

    internal const String DLL_NAME = "core_interop.dll";

    [DllImport(DLL_NAME, CallingConvention = CallingConvention.Cdecl)]
    private static extern void* create_context();

    [DllImport(DLL_NAME, CallingConvention = CallingConvention.Cdecl)]
    private static extern void destroy_context(void* ctx);

    [DllImport(DLL_NAME, CallingConvention = CallingConvention.Cdecl)]
    private static extern FfiBuf get_graph(void* ctx);


    [DllImport(DLL_NAME, CallingConvention = CallingConvention.Cdecl)]
    private static extern FfiBuf get_func_lib(void* ctx);


    private static IntPtr _core_interop_handle = IntPtr.Zero;

    private static void LoadDll() {
        if (_core_interop_handle != IntPtr.Zero) return;

        var sw = Stopwatch.StartNew();

        var full_dll_path = Path.GetFullPath(DLL_NAME);

        _core_interop_handle = LoadLibrary(full_dll_path);
        if (_core_interop_handle == IntPtr.Zero) {
            int error_code = Marshal.GetLastWin32Error();
            throw new Exception(string.Format("Failed to load library (ErrorCode: {0})", error_code));
        }

        var elapsed = sw.ElapsedMilliseconds;
    }

    private static void UnloadDll() {
        if (_core_interop_handle == IntPtr.Zero) return;

        FreeLibrary(_core_interop_handle);
        _core_interop_handle = IntPtr.Zero;
    }


    private readonly void* _ctx = null;

    static ScenariumCore() {
        LoadDll();
    }

    public ScenariumCore() {
        _ctx = create_context();
    }

    ~ScenariumCore() {
        destroy_context(_ctx);
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
}