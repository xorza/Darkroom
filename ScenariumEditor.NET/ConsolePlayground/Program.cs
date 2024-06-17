using CoreInterop;
using YamlDotNet.Serialization;
using YamlDotNet.Serialization.TypeInspectors;


var scenarium = new ScenariumCore();

var graph_yaml = scenarium.GetGraph();
Console.WriteLine(graph_yaml);

var func_lib_yaml = scenarium.GetFuncLib();