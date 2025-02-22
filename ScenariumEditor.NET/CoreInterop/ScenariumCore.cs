using System.Diagnostics;
using System.Runtime.InteropServices;
using CoreInterop.Utils;
using YamlDotNet.Serialization;
using YamlDotNet.Serialization.NamingConventions;

namespace CoreInterop;

public unsafe partial class ScenariumCore : IDisposable {
    internal const string DLL_NAME = "core_interop.dll";

    [LibraryImport(DLL_NAME)]
    [UnmanagedCallConv(CallConvs = [typeof(System.Runtime.CompilerServices.CallConvCdecl)])]
    private static partial void* create_context();

    [LibraryImport(DLL_NAME)]
    [UnmanagedCallConv(CallConvs = [typeof(System.Runtime.CompilerServices.CallConvCdecl)])]
    private static partial void destroy_context(void* ctx);


    [LibraryImport(DLL_NAME)]
    [UnmanagedCallConv(CallConvs = [typeof(System.Runtime.CompilerServices.CallConvCdecl)])]
    private static partial FfiBufInternal get_graph(void* ctx);

    [LibraryImport(DLL_NAME)]
    [UnmanagedCallConv(CallConvs = [typeof(System.Runtime.CompilerServices.CallConvCdecl)])]
    private static partial FfiBufInternal get_func_lib(void* ctx);

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate void CallbackDelegate(int value);
    
    [LibraryImport(DLL_NAME)]
    [UnmanagedCallConv(CallConvs = [typeof(System.Runtime.CompilerServices.CallConvCdecl)])]
    private static partial void register_callback(CallbackDelegate callback);

    private static void Callback(int value) {
        Console.WriteLine($"Callback received from Rust: {value}");
    }

    private void* _ctx = null;

    public ScenariumCore() {
        _ctx = create_context();

        var callback = new CallbackDelegate(Callback);
        register_callback(callback);
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
        using FfiBuf buf = get_graph(_ctx);
        var yaml = buf.ToString();

        return _deserializer.Deserialize<Graph>(new StringReader(yaml));
    }

    public FuncLib GetFuncLib() {
        using FfiBuf buf = get_func_lib(_ctx);
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
    }

    public void Dispose() {
        ReleaseUnmanagedResources();
        GC.SuppressFinalize(this);
    }
}