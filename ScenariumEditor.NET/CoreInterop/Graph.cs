using CoreInterop.Utils;
using YamlDotNet.Serialization;

namespace CoreInterop;

public class Graph {
    [YamlMember(Alias = "nodes")]
    public List<Node> Nodes { get; set; } = new();
}

public class Node {
    [YamlMember(Alias = "id")]
    public Uuid Id { get; set; } = new();

    [YamlMember(Alias = "func_id")]
    public Uuid FuncId { get; set; } = new();

    [YamlMember(Alias = "name")]
    public String Name { get; set; } = "";

    [YamlMember(Alias = "is_output")]
    public bool IsOutput { get; set; } = false;

    [YamlMember(Alias = "cache_outputs")]
    public bool CacheOutputs { get; set; } = false;

    [YamlMember(Alias = "inputs")]
    public List<NodeInput> Inputs { get; set; } = new();

    [YamlMember(Alias = "events")]
    public List<NodeEvent> Events { get; set; } = new();
}

public class NodeInput {
    [YamlMember(Alias = "binding")]
    public Binding Binding { get; set; } = new();

    [YamlMember(Alias = "const_value")]
    public Value ConstValue { get; set; } = null;
}

public class NodeEvent {
    [YamlMember(Alias = "subscribers")]
    public List<Uuid> Subscribers { get; set; } = new();
}

public enum BindingType {
    None,
    Const,
    Output,
}

public class Binding {
    public virtual BindingType Type => BindingType.None;
}

public class ConstBinding : Binding {
    public override BindingType Type => BindingType.Const;
}

public class OutputBinding : Binding {
    public override BindingType Type => BindingType.Output;

    [YamlMember(Alias = "output_node_id")]
    public Uuid OutputNodeId { get; set; } = new();

    [YamlMember(Alias = "output_index")]
    public UInt32 OutputIndex { get; set; } = new();
}