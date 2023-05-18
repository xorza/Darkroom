using System.Diagnostics;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace csso.Graph;

public struct DataType {
    /**/
}

public struct Output {
    public Int32 SelfIndex { get; set; }
    public Int32 NodeIndex { get; set; }
    public String Name { get; set; }

    public DataType Type { get; set; }
}

public struct Input {
    public Int32 SelfIndex { get; set; }
    public Int32 NodeIndex { get; set; }

    public String Name { get; set; }

    public bool Required { get; set; }
    public DataType Type { get; set; }
}

public enum NodeBehavior {
    Active,
    Passive
}

public struct Node {
    public Int32 SelfIndex { get; set; }
    public String Name { get; set; }

    [JsonConverter(typeof(JsonStringEnumConverter))]
    public NodeBehavior Behavior { get; set; }

    public bool IsOutput { get; set; }
}

public enum EdgeBehavior {
    ALways,
    Once
}

public struct Edge {
    public Int32 SelfIndex { get; set; }
    public Int32 OutputIndex { get; set; }
    public Int32 InputIndex { get; set; }

    [JsonConverter(typeof(JsonStringEnumConverter))]
    public EdgeBehavior Behavior { get; set; }
}

public class Graph {
    public Graph() { }
    public List<Output> Outputs { get; set; } = new();
    public List<Input> Inputs { get; set; } = new();
    public List<Node> Nodes { get; set; } = new();
    public List<Edge> Edges { get; set; } = new();

    public static Graph? FromJsonFile(String filename) {
        string jsonString = File.ReadAllText(filename);
        Graph? result = JsonSerializer.Deserialize<Graph>(jsonString);
        return result;
    }

    public String ToJson() {
        string jsonString = JsonSerializer.Serialize(this);
        return jsonString;
    }

    public void NewNode(ref Node node) {
        node.SelfIndex = Nodes.Count;
        Nodes.Add(node);
    }

    public void NewOutput(Int32 nodeIndex, ref Output output) {
        output.SelfIndex = Outputs.Count;
        output.NodeIndex = nodeIndex;
        Outputs.Add(output);
    }

    public void NewInput(Int32 nodeIndex, ref Input input) {
        input.SelfIndex = Inputs.Count;
        input.NodeIndex = nodeIndex;
        Inputs.Add(input);
    }

    public void NewEdge(ref Edge edge) {
        {
            //some asserts
            var input = Inputs[edge.InputIndex];
            var output = Outputs[edge.OutputIndex];

            Debug.Assert(Inputs[input.SelfIndex].Equals(input));
            Debug.Assert(Outputs[output.SelfIndex].Equals(output));

            Debug.Assert(output.Type.Equals(input.Type));
        }

        var inputIndex = edge.InputIndex;
        Edges.RemoveAll(_ => _.InputIndex == inputIndex);

        edge.SelfIndex = Edges.Count;
        Edges.Add(edge);
    }


    public List<Input> InputsForNode(Int32 nodeIndex) {
        return
            Inputs
                .Where(_ => _.NodeIndex == nodeIndex)
                .ToList();
    }

    public Edge? EdgeForInput(Int32 inputIndex) {
        return Edges.SingleOrDefault(_ => _.InputIndex == inputIndex);
    }
}