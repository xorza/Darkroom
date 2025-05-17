using System.Diagnostics;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;
using GraphLib.Utils;
using GraphLib.ViewModel;

namespace GraphLib.Controls;

public partial class GraphControl : UserControl {
    private MainWindowViewModel _view_model = null;

    public GraphControl() {
        InitializeComponent();
    }


    private void CanvasgBg_OnLoaded(object sender, RoutedEventArgs e) {
        _view_model = (MainWindowViewModel)DataContext;

        var graph_canvas = (UIElement)sender;

        graph_canvas.MouseDown += GraphCanvasBg_OnButtonDown;
        graph_canvas.MouseUp += GraphCanvasBg_OnButtonUp;
        graph_canvas.MouseMove += GraphCanvasBg_OnMouseMove;
        graph_canvas.MouseWheel += CanvasgBg_OnMouseWheel;
    }

    #region graph canvas events

    enum CanvasState {
        Idle,
        Dragging,
        StartCuttingConnections,
        CuttingConnections,
        NewConnection
    };

    private CanvasState _canvas_state = CanvasState.Idle;
    private Point _current_mouse_canvas_position;
    private Point _canvas_mouse_position_with_offset;

    private void GraphCanvasBg_OnButtonDown(object sender, MouseButtonEventArgs e) {
        _current_mouse_canvas_position = e.GetPosition(GraphCanvasBg);
        _canvas_mouse_position_with_offset = _current_mouse_canvas_position - _view_model.CanvasPosition.ToVector();
        e.Handled = true;

        switch (_canvas_state) {
            case CanvasState.Idle:
                // dragging canvas
                if (e.ChangedButton == MouseButton.Middle) {
                    if (StartCanvasDragging()) {
                        _canvas_state = CanvasState.Dragging;
                    }

                    return;
                }

                // cutting connections
                if (e.ChangedButton == MouseButton.Left) {
                    if (StartCuttingConnections()) {
                        _canvas_state = CanvasState.StartCuttingConnections;
                    }

                    return;
                }

                return;

            case CanvasState.Dragging:
                StopCanvasDragging();
                _canvas_state = CanvasState.Idle;
                return;

            case CanvasState.StartCuttingConnections:
                CancelCuttingConnections();
                _canvas_state = CanvasState.Idle;
                return;

            case CanvasState.CuttingConnections:
                CancelCuttingConnections();
                _canvas_state = CanvasState.Idle;
                return;

            case CanvasState.NewConnection:
                if (_nearest_pin != null) {
                    e.Handled = true;
                    CreateNewConnection(_active_pin, _nearest_pin);
                }

                return;

            default:
                throw new ArgumentOutOfRangeException();
        }
    }

    private void GraphCanvasBg_OnButtonUp(object sender, MouseButtonEventArgs e) {
        _current_mouse_canvas_position = e.GetPosition(GraphCanvasBg);
        _canvas_mouse_position_with_offset = _current_mouse_canvas_position - _view_model.CanvasPosition.ToVector();
        e.Handled = true;

        switch (_canvas_state) {
            case CanvasState.Idle:
                break;

            case CanvasState.Dragging:
                StopCanvasDragging();
                _canvas_state = CanvasState.Idle;
                return;

            case CanvasState.StartCuttingConnections:
                CancelCuttingConnections();

                // deselect node
                if (e.ChangedButton == MouseButton.Left) {
                    _view_model.SelectedNode = null;
                }

                _canvas_state = CanvasState.Idle;
                return;

            case CanvasState.CuttingConnections:
                if (e.ChangedButton == MouseButton.Left) {
                    ApplyCuttingConnections();
                } else {
                    CancelCuttingConnections();
                }

                _canvas_state = CanvasState.Idle;
                return;

            case CanvasState.NewConnection:
                e.Handled = true;
                if (_nearest_pin != null) {
                    CreateNewConnection(_active_pin, _nearest_pin);
                } else {
                    CancelNewConnection();
                }

                return;

            default:
                throw new ArgumentOutOfRangeException();
        }
    }

    private void GraphCanvasBg_OnMouseMove(object sender, MouseEventArgs e) {
        _current_mouse_canvas_position = e.GetPosition(GraphCanvasBg);
        _canvas_mouse_position_with_offset = _current_mouse_canvas_position - _view_model.CanvasPosition.ToVector();
        e.Handled = true;

        switch (_canvas_state) {
            case CanvasState.Idle:
                break;

            case CanvasState.Dragging:
                this._view_model.CanvasPosition = _current_mouse_canvas_position - _canvas_drag_start_mouse_position;
                return;

            case CanvasState.StartCuttingConnections:
                if (ContinueCuttingConnections()) {
                    _canvas_state = CanvasState.CuttingConnections;
                }

                return;

            case CanvasState.CuttingConnections:
                ContinueCuttingConnections();
                return;

            case CanvasState.NewConnection:
                ContinueNewConnection();
                return;

            default:
                throw new ArgumentOutOfRangeException();
        }
    }

    private void CanvasgBg_OnMouseWheel(object sender, MouseWheelEventArgs e) {
        var canvas = (FrameworkElement)sender;
        var delta = e.Delta;
        this._view_model.CanvasScale += delta / 2000.0;
    }

    #endregion

    #region canvas dragging

    private Vector _canvas_drag_start_mouse_position;

    private bool StartCanvasDragging() {
        if (!GraphCanvasBg.CaptureMouse()) return false;

        _canvas_drag_start_mouse_position = _current_mouse_canvas_position - this._view_model.CanvasPosition;
        return true;
    }

    private void StopCanvasDragging() {
        GraphCanvasBg.ReleaseMouseCapture();
    }

    #endregion

    #region new connection

    const double PIN_CONNECTION_DISTANCE = 80;

    private Pin _active_pin = null;
    private Pin _temp_mouse_pin = null;
    private Pin _nearest_pin = null;

    private void Node_OnPinClick(object sender, Pin e) {
        switch (_canvas_state) {
            case CanvasState.Dragging:
            case CanvasState.StartCuttingConnections:
            case CanvasState.CuttingConnections:
            case CanvasState.Idle:
                CancelCuttingConnections();
                StopCanvasDragging();

                _active_pin = e;
                _temp_mouse_pin = new Pin {
                    DataType = e.DataType,
                    PinType = e.PinType.GetOpposite(),
                    CanvasPosition = _canvas_mouse_position_with_offset
                };
                NewConnectionControl.Visibility = Visibility.Visible;

                NewConnectionControl.DataContext = new Connection(_active_pin, _temp_mouse_pin);
                // NewConnectionControl.CaptureMouse();

                _canvas_state = CanvasState.NewConnection;
                return;

            case CanvasState.NewConnection:
                CreateNewConnection(_active_pin, e);

                return;

            default:
                throw new ArgumentOutOfRangeException();
        }
    }

    private void ContinueNewConnection() {
        Debug.Assert(_temp_mouse_pin != null);

        _temp_mouse_pin.CanvasPosition = _canvas_mouse_position_with_offset;
        _nearest_pin = _view_model.Nodes
            .SelectMany(node => node.Events
                .Concat(node.Inputs)
                .Concat(node.Outputs)
                .Concat([node.Trigger])
            )
            .Where(pin => pin.PinType == _temp_mouse_pin.PinType && pin.DataType == _temp_mouse_pin.DataType)
            .Where(pin => (pin.CanvasPosition - _temp_mouse_pin.CanvasPosition).LengthSquared < PIN_CONNECTION_DISTANCE)
            .MinBy(pin => (pin.CanvasPosition - _temp_mouse_pin.CanvasPosition).LengthSquared);
        if (_nearest_pin != null) {
            _temp_mouse_pin.CanvasPosition = _nearest_pin.CanvasPosition;
        }
    }

    private void CancelNewConnection() {
        _temp_mouse_pin = null;
        NewConnectionControl.Visibility = Visibility.Collapsed;
        NewConnectionControl.DataContext = null;
        _canvas_state = CanvasState.Idle;
    }

    private void CreateNewConnection(Pin p1, Pin p2) {
        if (p1.DataType == p2.DataType && p1.PinType.GetOpposite() == p2.PinType) {
            _view_model.Connections.Add(new Connection(p1, p2));
            CancelNewConnection();
        }
    }

    #endregion

    #region cut connections

    private static Point[] GetIntersectionPoints(Geometry g1, Geometry g2) {
        Geometry og1 = g1.GetWidenedPathGeometry(new Pen(Brushes.Black, 1.0));
        Geometry og2 = g2.GetWidenedPathGeometry(new Pen(Brushes.Black, 1.0));

        CombinedGeometry cg = new CombinedGeometry(GeometryCombineMode.Intersect, og1, og2);

        PathGeometry pg = cg.GetFlattenedPathGeometry();
        Point[] result = new Point[pg.Figures.Count];
        for (int i = 0; i < pg.Figures.Count; i++) {
            Rect fig = new PathGeometry(new PathFigure[] { pg.Figures[i] }).Bounds;
            result[i] = new Point(fig.Left + fig.Width / 2.0, fig.Top + fig.Height / 2.0);
        }

        return result;
    }

    private Point _previous_canvas_mouse_position;
    private PathGeometry _cutting_path_geometry;
    private PathFigure _cutting_path_figure;

    private readonly List<ConnectionControl> _connection_controls = new();

    private bool StartCuttingConnections() {
        if (!GraphCanvasBg.CaptureMouse())
            return false;

        _previous_canvas_mouse_position = _current_mouse_canvas_position - _view_model.CanvasPosition.ToVector();

        _cutting_path_figure = new PathFigure(
            new Point(),
            [new LineSegment(_previous_canvas_mouse_position, false)],
            false
        );

        CuttingPath.Data = _cutting_path_geometry = new PathGeometry([_cutting_path_figure]);
        CuttingPath.Visibility = Visibility.Visible;
        return true;
    }

    private bool ContinueCuttingConnections() {
        var delta = _canvas_mouse_position_with_offset - _previous_canvas_mouse_position;
        if (delta.Length < 4) return false;

        _cutting_path_figure.Segments.Add(
            new LineSegment(_canvas_mouse_position_with_offset, true)
        );
        _previous_canvas_mouse_position = _canvas_mouse_position_with_offset;

        foreach (var connection_control in _connection_controls) {
            var points = GetIntersectionPoints(_cutting_path_geometry, connection_control.Geometry);
            if (points.Any()) {
                connection_control.IsDeleted = true;
            }
        }

        return true;
    }

    private void ApplyCuttingConnections() {
        GraphCanvasBg.ReleaseMouseCapture();

        CuttingPath.Visibility = Visibility.Collapsed;
        CuttingPath.Data = null;
        _cutting_path_figure = null;

        foreach (var connection_control in _connection_controls.Where(ctrl => ctrl.IsDeleted)) {
            var connection = (Connection)connection_control.DataContext;
            _view_model.Connections.Remove(connection);
        }
    }

    private void CancelCuttingConnections() {
        GraphCanvasBg.ReleaseMouseCapture();

        CuttingPath.Visibility = Visibility.Collapsed;
        CuttingPath.Data = null;
        _cutting_path_figure = null;

        _connection_controls.ForEach(ctrl => ctrl.IsDeleted = false);
    }

    private void ConnectionControl_OnLoaded(object sender, RoutedEventArgs e) {
        var connection_control = (ConnectionControl)sender;
        _connection_controls.Add(connection_control);
    }

    private void ConnectionControl_OnUnloaded(object sender, RoutedEventArgs e) {
        var connection_control = (ConnectionControl)sender;
        _connection_controls.Remove(connection_control);
    }

    #endregion

    private void Node_OnDeletePressed(object sender, EventArgs e) {
        var node_control = (NodeControl)sender;
        var node = (Node)node_control.DataContext;
        _view_model.Remove(node);
    }

    private void Node_OnSelected(object sender, EventArgs e) {
        var node_control = (NodeControl)sender;
        var node = (Node)node_control.DataContext;
        _view_model.SelectedNode = node;
    }
}