using CoreInterop;


var scenarium = new ScenariumCore();

var graph_yaml = scenarium.GetGraph();
Console.WriteLine(graph_yaml);

var func_lib_yaml = scenarium.GetFuncLib();
Console.WriteLine(func_lib_yaml);
