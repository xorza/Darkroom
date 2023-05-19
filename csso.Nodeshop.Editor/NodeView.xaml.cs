namespace csso.Nodeshop.Editor;

public partial class NodeView : ContentView {
    private VisualNode _node = null!;

    public NodeView() {
        InitializeComponent();
    }

    public VisualNode Node {
        get => _node;
        set {
            if (_node == value) {
                return;
            }

            _node = value;
            BindingContext = _node;
        }
    }
}