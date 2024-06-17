using CoreInterop;
using CoreInterop.Utils;

namespace InteropTests;

public class Tests {
    [SetUp]
    public void Setup() {
    }

    [Test]
    public void UuidTest() {
        var uuid = Uuid.NewV4();
        var uuid_str = uuid.ToString();
        var uuid2 = Uuid.FromString(uuid_str);
        
        Assert.That(uuid2, Is.EqualTo(uuid));
        Assert.Pass();
    }

    [Test]
    public void FuncDeserialization() {
        var scenarium = new ScenariumCore();
        var func_lib = scenarium.GetFuncLib();
        
        Assert.That(func_lib.Funcs.Count, Is.GreaterThan(0));
        Assert.Pass();
    }
    
    [Test]
    public void NodesDeserialization() {
        var scenarium = new ScenariumCore();
        var graph = scenarium.GetGraph();
        
        Assert.That(graph.Nodes.Count, Is.EqualTo(5));
        Assert.Pass();
    }
}