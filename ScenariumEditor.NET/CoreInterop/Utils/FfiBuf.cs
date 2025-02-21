using System.Runtime.InteropServices;

namespace CoreInterop.Utils;

internal readonly unsafe struct FfiBuf : IDisposable {
    readonly byte* _bytes = null;
    readonly uint _length = 0;
    readonly uint _capacity = 0;


    [DllImport(ScenariumCore.DLL_NAME, CallingConvention = CallingConvention.Cdecl)]
    private static extern void destroy_ffi_buf(FfiBuf buf);

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
            destroy_ffi_buf(this);
        }
    }
}