using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;

namespace CoreInterop;


internal static unsafe partial class CoreNative {
    [DllImport("kernel32.dll", CharSet = CharSet.Auto, SetLastError = true)]
    private static extern IntPtr LoadLibrary(string libname);

    [DllImport("kernel32.dll", CharSet = CharSet.Auto)]
    private static extern bool FreeLibrary(IntPtr h_module);


    private static IntPtr _core_interop_handle;

    public static void LoadDll() {
        if (_core_interop_handle != IntPtr.Zero) return;

        var sw = Stopwatch.StartNew();

        var full_dll_path = Path.GetFullPath(__DllName + ".dll");

        _core_interop_handle = LoadLibrary(full_dll_path);
        if (_core_interop_handle == IntPtr.Zero) {
            int error_code = Marshal.GetLastWin32Error();
            throw new Exception(string.Format("Failed to load library (ErrorCode: {0})", error_code));
        }

        var elapsed = sw.ElapsedMilliseconds;
    }

    public static void UnloadDll() {
        if (_core_interop_handle == IntPtr.Zero) return;

        FreeLibrary(_core_interop_handle);
        _core_interop_handle = IntPtr.Zero;
    }

    public static bool IsLoaded {
        get { return _core_interop_handle != IntPtr.Zero; }
    }
}


internal unsafe partial struct FfiBuf : IDisposable {
    public static FfiBuf FromString(String s) {
        var bytes = Marshal.StringToHGlobalAnsi(s);
        return new FfiBuf {
            bytes = (byte*)bytes,
            length = (uint)s.Length,
            capacity = (uint)s.Length
        };
    }

    public static FfiBuf FromArray(byte[] array) {
        var bytes = Marshal.AllocHGlobal(array.Length);
        Marshal.Copy(array, 0, bytes, array.Length);
        return new FfiBuf {
            bytes = (byte*)bytes,
            length = (uint)array.Length,
            capacity = (uint)array.Length
        };
    }

    public static FfiBuf FromList(List<byte> list) {
        return FromArray(list.ToArray());
    }

    public override String ToString() {
        if (bytes == null) throw new InvalidOperationException("Disposed buffer");
        return Marshal.PtrToStringAnsi((IntPtr)bytes, (int)length);
    }

    public byte[] ToArray() {
        if (bytes == null) throw new InvalidOperationException("Disposed buffer");

        var result = new byte[length];
        Marshal.Copy((IntPtr)bytes, result, 0, (int)length);
        return result;
    }

    public List<byte> ToList() {
        return this.ToArray().ToList();
    }

    public T[] ToArray<T>() where T : unmanaged {
        if (bytes == null) throw new InvalidOperationException("Disposed buffer");


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
        if (this.length % t_size != 0) throw new InvalidOperationException("Invalid array size");

        var t_length = (int)this.length / t_size;
        var result = new T[t_length];

        if (t_length == 0) return result;

        GCHandle handle = new GCHandle();
        try {
            // Make sure the array won't be moved around by the GC 
            handle = GCHandle.Alloc(result, GCHandleType.Pinned);
            var destination = handle.AddrOfPinnedObject().ToPointer();
            Buffer.MemoryCopy(bytes, destination, length, length);
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
        if (bytes != null) {
            Marshal.FreeHGlobal((IntPtr)bytes);
            bytes = null;
        }
    }
}

internal unsafe partial struct FfiId : IDisposable {
    public readonly Guid ToGuid() {
        // byte[] guidData = new byte[16];
        // Array.Copy(BitConverter.GetBytes(Item1), guidData, 8);
        // Array.Copy(BitConverter.GetBytes(Item2), 0, guidData, 8, 8);
        // return new Guid(guidData);

        return new Guid(Item1.ToString());
    }

    public void Dispose() {
        Item1.Dispose();
    }
}

internal unsafe partial struct FfiStr : IDisposable {
    public override String ToString() {
        return Item1.ToString();
    }

    public FfiStr FromString(String s) {
        return new FfiStr {
            Item1 = FfiBuf.FromString(s)
        };
    }

    public void Dispose() {
        Item1.Dispose();
    }
}

internal unsafe partial struct FfiStrVec : IDisposable {
    public readonly String[] ToStringArray() {
        var reader = new BinaryReader(new UnmanagedMemoryStream(Item1.bytes, Item1.length));
        
        var count = reader.ReadUInt32();
        var result = new String[count];
        for (var i = 0; i < count; i++) {
            var length = reader.ReadUInt32();
            result[i] = Encoding.UTF8.GetString(reader.ReadBytes((int)length));
        }

        return result;
    }

    public void Dispose() {
        Item1.Dispose();
    }
}

internal unsafe partial struct FfiNode : IDisposable {
    public void Dispose() {
        id.Dispose();
        func_id.Dispose();
        name.Dispose();
        inputs.Dispose();
        events.Dispose();
    }
}

internal unsafe partial struct FfiFunc : IDisposable {
    public void Dispose() {
        id.Dispose();
        name.Dispose();
        category.Dispose();
        inputs.Dispose();
        outputs.Dispose();
        events.Dispose();
    }
}