using System.ComponentModel.Design;
using CoreInterop.Utils;
using YamlDotNet.Serialization;

namespace CoreInterop;

public class FuncLib {
    [YamlMember(Alias = "funcs")]
    public List<Func> Funcs { get; set; }
}

public enum FuncBehavior {
    Active,
    Passive
}


public class ValueVariant {
    [YamlMember(Alias = "name")]
    public String Name { get; set; } = "";

    [YamlMember(Alias = "value")]
    public Value Value { get; set; } = null;
}

public class FuncInput {
    [YamlMember(Alias = "name")]
    public String Name { get; set; } = "";

    [YamlMember(Alias = "is_required")]
    public bool IsRequired { get; set; } = false;

    [YamlMember(Alias = "data_type")]
    public DataType DataType { get; set; } = null;

    [YamlMember(Alias = "default_value")]
    public Value DefaultValue { get; set; } = null;

    [YamlMember(Alias = "variants")]
    public List<ValueVariant> Variants { get; set; } = new();
}

public class FuncOutput {
    [YamlMember(Alias = "name")]
    public String Name { get; set; } = "";

    [YamlMember(Alias = "data_type")]
    public DataType DataType { get; set; } = null;
}

public class FuncEvent {
    [YamlMember(Alias = "name")]
    public String Name { get; set; } = "";
}

public class Func {
    [YamlMember(Alias = "id")]
    public Uuid Id { get; set; } = new();

    [YamlMember(Alias = "name")]
    public String Name { get; set; } = "";

    [YamlMember(Alias = "description")]
    public String Description { get; set; } = "";

    [YamlMember(Alias = "category")]
    public String Category { get; set; } = "";

    [YamlMember(Alias = "behavior")]
    public FuncBehavior Behavior { get; set; } = FuncBehavior.Active;

    [YamlMember(Alias = "is_output")]
    public bool IsOutput { get; set; } = false;

    [YamlMember(Alias = "inputs")]
    public List<FuncInput> Inputs { get; set; } = new();

    [YamlMember(Alias = "outputs")]
    public List<FuncOutput> Outputs { get; set; } = new();

    [YamlMember(Alias = "events")]
    public List<FuncEvent> Events { get; set; } = new();
}