using System.ComponentModel.Design;
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

public class FuncInput {
    [YamlMember(Alias = "name")]
    public String Name { get; set; }

    [YamlMember(Alias = "is_required")]
    public bool IsRequired { get; set; }
    //...
}

public class FuncOutput {
    [YamlMember(Alias = "name")]
    public String Name { get; set; }
    // data type
}

public class FuncEvent {
    [YamlMember(Alias = "name")]
    public String Name { get; set; }
}

public class Func {
    [YamlMember(Alias = "id")]
    public String Id { get; set; }

    [YamlMember(Alias = "name")]
    public String Name { get; set; }

    [YamlMember(Alias = "description")]
    public String Description { get; set; }

    [YamlMember(Alias = "category")]
    public String Category { get; set; }

    [YamlMember(Alias = "behavior")]
    public String Behavior { get; set; }

    [YamlMember(Alias = "is_output")]
    public String IsOutput { get; set; }

    [YamlMember(Alias = "inputs")]
    public List<FuncInput> Inputs { get; set; }

    [YamlMember(Alias = "outputs")]
    public List<FuncOutput> Outputs { get; set; }

    [YamlMember(Alias = "events")]
    public List<FuncEvent> Events { get; set; }
}