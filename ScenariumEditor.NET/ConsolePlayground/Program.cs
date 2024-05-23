using System.Diagnostics;
using CoreInterop;


ScenariumCore.Init();

for (int i = 0; i < 3; i++) {
    var scenarium = new ScenariumCore();
    // scenarium.GetNodes();
    scenarium.GetFuncs();
}