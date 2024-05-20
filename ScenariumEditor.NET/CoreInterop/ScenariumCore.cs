using CsBindgen;

namespace CoreInterop;

public class ScenariumCore {
    public static UInt32 Test()  {
        // Call the generated method
       return NativeMethods.add(1, 2);
    }
}