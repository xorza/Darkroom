using System.Diagnostics;
using System.Runtime.InteropServices;

namespace CoreInterop;

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
        return Marshal.PtrToStringAnsi((IntPtr)bytes, (int)length);
    }

    public byte[] ToArray() {
        var result = new byte[length];
        Marshal.Copy((IntPtr)bytes, result, 0, (int)length);
        return result;
    }

    public List<byte> ToList() {
        return this.ToArray().ToList();
    }

    public T[] ToArray<T>() where T : unmanaged {
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

        var tSize = Marshal.SizeOf<T>();
        if (this.length % tSize != 0) throw new InvalidOperationException("Invalid array size");

        var tLength = (int)this.length / tSize;
        var result = new T[tLength];

        if (tLength == 0) return result;

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

internal static unsafe partial class CoreNative {
    [DllImport("kernel32.dll", CharSet = CharSet.Auto, SetLastError = true)]
    private static extern IntPtr LoadLibrary(string libname);

    [DllImport("kernel32.dll", CharSet = CharSet.Auto)]
    private static extern bool FreeLibrary(IntPtr hModule);


    private static IntPtr _coreInteropHandle;

    public static void LoadDll() {
        if (_coreInteropHandle != IntPtr.Zero) return;

        var sw = Stopwatch.StartNew();

        var fullDllPath = Path.GetFullPath(__DllName + ".dll");

        _coreInteropHandle = LoadLibrary(fullDllPath);
        if (_coreInteropHandle == IntPtr.Zero) {
            int errorCode = Marshal.GetLastWin32Error();
            throw new Exception(string.Format("Failed to load library (ErrorCode: {0})", errorCode));
        }

        var elapsed = sw.ElapsedMilliseconds;
    }

    public static void UnloadDll() {
        if (_coreInteropHandle == IntPtr.Zero) return;

        FreeLibrary(_coreInteropHandle);
        _coreInteropHandle = IntPtr.Zero;
    }

    public static bool IsLoaded {
        get { return _coreInteropHandle != IntPtr.Zero; }
    }
}

internal unsafe partial struct Id {
    public Guid ToGuid() {
        // byte[] guidData = new byte[16];
        // Array.Copy(BitConverter.GetBytes(Item1), guidData, 8);
        // Array.Copy(BitConverter.GetBytes(Item2), 0, guidData, 8, 8);
        // return new Guid(guidData);
        
        return new Guid(Item1.ToString());
    }
}