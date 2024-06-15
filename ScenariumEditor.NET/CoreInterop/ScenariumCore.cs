using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using YamlDotNet.Serialization;
using YamlDotNet.Serialization.NamingConventions;

namespace CoreInterop;

public unsafe class ScenariumCore {
    [DllImport("kernel32.dll", CharSet = CharSet.Auto, SetLastError = true)]
    private static extern IntPtr LoadLibrary(string libname);

    [DllImport("kernel32.dll", CharSet = CharSet.Auto)]
    private static extern bool FreeLibrary(IntPtr h_module);

    private const String DLL_NAME = "core_interop.dll";

    [DllImport(DLL_NAME, CallingConvention = CallingConvention.Cdecl)]
    private static extern void* create_context();

    [DllImport(DLL_NAME, CallingConvention = CallingConvention.Cdecl)]
    private static extern void destroy_context(void* ctx);

    [DllImport(DLL_NAME, CallingConvention = CallingConvention.Cdecl)]
    private static extern FfiBuf get_graph(void* ctx);

    [DllImport(DLL_NAME, CallingConvention = CallingConvention.Cdecl)]
    internal static extern void destroy_ffi_buf(FfiBuf buf);

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


    public string GetGraph() {
        using var buf = get_graph(_ctx);
        return buf.ToString();
    }

    public FuncLib GetFuncLib() {
        using var buf = get_func_lib(_ctx);
        var yaml = buf.ToString();

        var deserializer = new DeserializerBuilder()
            .WithNamingConvention(CamelCaseNamingConvention.Instance)
            .Build();

        var funcs = deserializer.Deserialize<List<Func>>(new StringReader(yaml));
        return new FuncLib() {
            Funcs = funcs
        };
    }
}

internal readonly unsafe struct FfiBuf : IDisposable {
    readonly byte* _bytes = null;
    readonly uint _length = 0;
    readonly uint _capacity = 0;

    public FfiBuf() {
    }

    public FfiBuf(String s) {
        var bytes = Marshal.StringToHGlobalAnsi(s);

        _bytes = (byte*)bytes;
        _length = (uint)s.Length;
        _capacity = (uint)s.Length;
    }

    public FfiBuf(byte[] array) {
        var bytes = Marshal.AllocHGlobal(array.Length);
        Marshal.Copy(array, 0, bytes, array.Length);

        _bytes = (byte*)bytes;
        _length = (uint)array.Length;
        _capacity = (uint)array.Length;
    }

    public FfiBuf(List<byte> list) {
        var array = list.ToArray();
        var bytes = Marshal.AllocHGlobal(array.Length);
        Marshal.Copy(array, 0, bytes, array.Length);

        _bytes = (byte*)bytes;
        _length = (uint)array.Length;
        _capacity = (uint)array.Length;
    }

    public override String ToString() {
        if (_bytes == null) throw new InvalidOperationException("Disposed buffer");
        return Marshal.PtrToStringAnsi((IntPtr)_bytes, (int)_length);
    }

    public byte[] ToArray() {
        if (_bytes == null) throw new InvalidOperationException("Disposed buffer");

        var result = new byte[_length];
        Marshal.Copy((IntPtr)_bytes, result, 0, (int)_length);
        return result;
    }

    public List<byte> ToList() {
        return this.ToArray().ToList();
    }

    public T[] ToArray<T>() where T : unmanaged {
        if (_bytes == null) throw new InvalidOperationException("Disposed buffer");


        var type = typeof(T);

        if (!type.IsPrimitive) {
            if (type.IsValueType) {
                if (!type.IsLayoutSequential && !type.IsExplicitLayout) {
                    throw new InvalidOperationException(string.Format("{0} does not define a StructLayout attribute",
                        type));
                }
            } else {
                throw new InvalidOperationException(string.Format("{0} is not a primitive or value type", type));
            }
        }

        var t_size = Marshal.SizeOf<T>();
        if (this._length % t_size != 0) throw new InvalidOperationException("Invalid array size");

        var t_length = (int)this._length / t_size;
        var result = new T[t_length];

        if (t_length == 0) return result;

        GCHandle handle = new GCHandle();
        try {
            // Make sure the array won't be moved around by the GC 
            handle = GCHandle.Alloc(result, GCHandleType.Pinned);
            var destination = handle.AddrOfPinnedObject().ToPointer();
            Buffer.MemoryCopy(_bytes, destination, _length, _length);
        } finally {
            if (handle.IsAllocated)
                handle.Free();
        }

        return result;
    }

    public List<T> ToList<T>() where T : unmanaged {
        return this.ToArray<T>().ToList();
    }

    public void Dispose() {
        if (_bytes != null) {
            ScenariumCore.destroy_ffi_buf(this);
        }
    }
}