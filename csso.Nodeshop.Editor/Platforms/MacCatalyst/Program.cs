using UIKit;

namespace csso.Nodeshop.Editor;

public class Program {
    static void Main(string[] args) {
#if DEBUG
        //Rider debugger doesn't work without this
        Thread.Sleep(3500);
#endif

        UIApplication.Main(args, null, typeof(AppDelegate));
    }
}