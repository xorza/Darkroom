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

    public void Dispose() {
        if (bytes != null) {
            Marshal.FreeHGlobal((IntPtr)bytes);
            bytes = null;
        }
    }
}

internal static unsafe partial class CoreNative {
    public static void Test() {
        var buf = test3();
        var result = buf.ToString();
        buf.Dispose();
        Console.WriteLine(result);
    }
    
    
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