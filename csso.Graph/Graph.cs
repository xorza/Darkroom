using System.Diagnostics;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace csso.Graph;

public struct DataType {
    public static DataType FromType(Type type) {
        return new DataType();
    }
}

public struct Output {
    public int SelfIndex { get; set; }
    public int NodeIndex { get; set; }
    public string Name { get; set; }

    public DataType Type { get; set; }
}

public struct Input {
    public int SelfIndex { get; set; }
    public int NodeIndex { get; set; }

    public string Name { get; set; }

    public bool Required { get; set; }
    public DataType Type { get; set; }
}

public enum NodeBehavior {
    Active,
    Passive
}

public struct Node {
    public int SelfIndex { get; set; }
    public string Name { get; set; }

    [JsonConverter(typeof(JsonStringEnumConverter))]
    public NodeBehavior Behavior { get; set; }

    public bool IsOutput { get; set; }
}

public enum EdgeBehavior {
    Always,
    Once
}

public class Edge {
    public int SelfIndex { get; set; }
    public int OutputIndex { get; set; }
    public int InputIndex { get; set; }

    [JsonConverter(typeof(JsonStringEnumConverter))]
    public EdgeBehavior Behavior { get; set; }
}

public class Graph {
    public List<Output> Outputs { get; set; } = new();
    public List<Input> Inputs { get; set; } = new();
    public List<Node> Nodes { get; set; } = new();
    public List<Edge> Edges { get; set; } = new();

    public static Graph? FromJsonFile(string filename) {
        var jsonString = File.ReadAllText(filename);
        var result = JsonSerializer.Deserialize<Graph>(jsonString);
        return result;
    }

    public string ToJson() {
        var jsonString = JsonSerializer.Serialize(this);
        return jsonString;
    }

    public void NewNode(ref Node node) {
        node.SelfIndex = Nodes.Count;
        Nodes.Add(node);
    }

    public void NewOutput(int nodeIndex, ref Output output) {
        output.SelfIndex = Outputs.Count;
        output.NodeIndex = nodeIndex;
        Outputs.Add(output);
    }

    public void NewInput(int nodeIndex, ref Input input) {
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

        var existingEdge = Edges.SingleOrDefault(_ => _.InputIndex == inputIndex);
        if (existingEdge != null) {
            edge.SelfIndex = existingEdge.SelfIndex;
            Edges[edge.SelfIndex] = edge;
        }
        else {
            edge.SelfIndex = Edges.Count;
            Edges.Add(edge);
        }
    }


    public List<Input> InputsForNode(int nodeIndex) {
        return
            Inputs
                .Where(_ => _.NodeIndex == nodeIndex)
                .ToList();
    }

    public Edge? EdgeForInput(int inputIndex) {
        return Edges.SingleOrDefault(_ => _.InputIndex == inputIndex);
    }
}