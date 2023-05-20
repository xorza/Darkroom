namespace csso.Nodeshop.Editor;

public partial class MainPage : ContentPage {
    public MainPage() {
        Graph = VisualGraph.CreateTest();

        Loaded += OnLoaded;
        InitializeComponent();
    }


    public VisualGraph Graph { get; set; }

    private void OnLoaded(object sender, EventArgs e) {
        foreach (var vNode in Graph.Nodes) {
            var node = Graph.Graph.Graph.Nodes[vNode.NodeIndex];

            var nodeView = NodeAbsoluteLayout.Children.OfType<NodeView>()
                .SingleOrDefault(_ => _.VM.NodeIndex == vNode.NodeIndex);
            if (nodeView == null) {
                nodeView = new NodeView { };
                var nodeViewModel = new NodeViewModel() {
                    NodeIndex = vNode.NodeIndex,
                    Name = vNode.Name
                };
                var inputs = Graph.Graph.Graph.Inputs.Where(_ => _.NodeIndex == vNode.NodeIndex).Select(_ => _.Name)
                    .ToList();
                nodeViewModel.Inputs = inputs;
                var outputs = Graph.Graph.Graph.Outputs.Where(_ => _.NodeIndex == vNode.NodeIndex).Select(_ => _.Name)
                    .ToList();
                nodeViewModel.Outputs = outputs;

                nodeView.VM = nodeViewModel;
                NodeAbsoluteLayout.Children.Add(nodeView);
            }

            var sizeRequest = nodeView.Measure(double.PositiveInfinity, double.PositiveInfinity);
            AbsoluteLayout.SetLayoutBounds(nodeView,
                new Rect(vNode.Point.X, vNode.Point.Y, sizeRequest.Request.Width, sizeRequest.Request.Height));
        }
    }

    private void Button_OnClicked(object sender, EventArgs e) {
    }
}