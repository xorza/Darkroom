using System.Diagnostics;

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
            var nodeView = new NodeView();
            var sizeRequest = nodeView.Measure(double.PositiveInfinity, double.PositiveInfinity);
            AbsoluteLayout.SetLayoutBounds(nodeView,
                new Rect(vNode.Point.X, vNode.Point.Y, sizeRequest.Minimum.Width, sizeRequest.Minimum.Height));
            nodeView.Node = vNode;
            NodeAbsoluteLayout.Children.Add(nodeView);
        }
    }


    private void Button_OnClicked(object sender, EventArgs e) {
        Debug.WriteLine("Hello World!");
    }
}