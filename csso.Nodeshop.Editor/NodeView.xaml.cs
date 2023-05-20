namespace csso.Nodeshop.Editor;

public class NodeViewModel {
    public NodeViewModel() {
    }

    public Int32 NodeIndex { get; set; }

    public String Name { get; set; }
    public List<String> Inputs { get; set; } = new();
    public List<String> Outputs { get; set; } = new();
}

public partial class NodeView : ContentView {
    private NodeViewModel _vm = null!;

    public NodeView() {
        InitializeComponent();
        VerticalOptions = LayoutOptions.Start;
        HorizontalOptions = LayoutOptions.Start;
    }

    public NodeViewModel VM {
        get => _vm;
        set {
            if (_vm == value) {
                return;
            }

            _vm = value;
            BindingContext = _vm;
        }
    }
}