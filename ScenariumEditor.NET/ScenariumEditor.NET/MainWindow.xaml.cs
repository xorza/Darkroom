using System;
using System.Collections.Generic;
using System.Collections.Specialized;
using System.Diagnostics;
using System.Linq;
using System.Text;
using System.Threading.Tasks;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Data;
using System.Windows.Documents;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using System.Windows.Navigation;
using System.Windows.Shapes;
using GraphLib.Controls;
using GraphLib.Utils;
using GraphLib.ViewModel;

namespace ScenariumEditor.NET;

public partial class MainWindow : Window {
    private readonly MainWindowViewModel _viewModel = new DesignMainWindowViewModel();

    public MainWindow() {
        InitializeComponent();

        this.DataContext = _viewModel;
    }


    private void AddDesignNodeButton_OnClick(object sender, RoutedEventArgs e) {
        _viewModel.Nodes.Add(new DesignNode());
    }

    private void MainWindow_OnLoaded(object sender, RoutedEventArgs e) {
    }

    private void CanvasgBg_OnLoaded(object sender, RoutedEventArgs e) {
        var graphCanvas = (UIElement)sender;

        graphCanvas.MouseDown += GraphCanvasBg_OnButtonDown;
        graphCanvas.MouseUp += GraphCanvasBg_OnButtonUp;
        graphCanvas.MouseMove += GraphCanvasBg_OnMouseMove;
        graphCanvas.MouseWheel += CanvasgBg_OnMouseWheel;
        
    }

    #region graph canvas events

    enum CanvasState {
        Idle,
        Dragging,
        StartCuttingConnections,
        CuttingConnections,
        NewConnection
    };

    private CanvasState _canvasState = CanvasState.Idle;
    private Point _currentMouseCanvasPosition;
    private Point _canvasMousePositionWithOffset;

    private void GraphCanvasBg_OnButtonDown(object sender, MouseButtonEventArgs e) {
        _currentMouseCanvasPosition = e.GetPosition(GraphCanvasBg);
        _canvasMousePositionWithOffset = _currentMouseCanvasPosition - _viewModel.CanvasPosition.ToVector();
        e.Handled = true;

        switch (_canvasState) {
            case CanvasState.Idle:
                // dragging canvas
                if (e.ChangedButton == MouseButton.Middle) {
                    if (StartCanvasDragging()) {
                        _canvasState = CanvasState.Dragging;
                    }

                    return;
                }

                // cutting connections
                if (e.ChangedButton == MouseButton.Left) {
                    if (StartCuttingConnections()) {
                        _canvasState = CanvasState.StartCuttingConnections;
                    }

                    return;
                }

                return;

            case CanvasState.Dragging:
                StopCanvasDragging();
                _canvasState = CanvasState.Idle;
                return;

            case CanvasState.StartCuttingConnections:
                CancelCuttingConnections();
                _canvasState = CanvasState.Idle;
                return;

            case CanvasState.CuttingConnections:
                CancelCuttingConnections();
                _canvasState = CanvasState.Idle;
                return;

            case CanvasState.NewConnection:
                if (_nearestPin != null) {
                    _viewModel.Connections.Add(new Connection(_activePin, _nearestPin));
                    CancelNewConnection();
                    _canvasState = CanvasState.Idle;
                }

                return;

            default:
                throw new ArgumentOutOfRangeException();
        }
    }

    private void GraphCanvasBg_OnButtonUp(object sender, MouseButtonEventArgs e) {
        _currentMouseCanvasPosition = e.GetPosition(GraphCanvasBg);
        _canvasMousePositionWithOffset = _currentMouseCanvasPosition - _viewModel.CanvasPosition.ToVector();
        e.Handled = true;

        switch (_canvasState) {
            case CanvasState.Idle:
                break;

            case CanvasState.Dragging:
                StopCanvasDragging();
                _canvasState = CanvasState.Idle;
                return;

            case CanvasState.StartCuttingConnections:
                CancelCuttingConnections();

                // deselect node
                if (e.ChangedButton == MouseButton.Left) {
                    _viewModel.SelectedNode = null;
                }

                _canvasState = CanvasState.Idle;
                return;

            case CanvasState.CuttingConnections:
                if (e.ChangedButton == MouseButton.Left) {
                    ApplyCuttingConnections();
                } else {
                    CancelCuttingConnections();
                }
                
                _canvasState = CanvasState.Idle;
                return;

            case CanvasState.NewConnection:
                CancelNewConnection();
                _canvasState = CanvasState.Idle;
                return;

            default:
                throw new ArgumentOutOfRangeException();
        }
    }

    private void GraphCanvasBg_OnMouseMove(object sender, MouseEventArgs e) {
        _currentMouseCanvasPosition = e.GetPosition(GraphCanvasBg);
        _canvasMousePositionWithOffset = _currentMouseCanvasPosition - _viewModel.CanvasPosition.ToVector();
        e.Handled = true;

        switch (_canvasState) {
            case CanvasState.Idle:
                break;

            case CanvasState.Dragging:
                this._viewModel.CanvasPosition = _currentMouseCanvasPosition - _canvasDragStartMousePosition;
                return;

            case CanvasState.StartCuttingConnections:
                if (ContinueCuttingConnections()) {
                    _canvasState = CanvasState.CuttingConnections;
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
        this._viewModel.CanvasScale += delta / 2000.0;
    }

    #endregion

    #region canvas dragging

    private Vector _canvasDragStartMousePosition;

    private bool StartCanvasDragging() {
        if (!GraphCanvasBg.CaptureMouse()) return false;

        _canvasDragStartMousePosition = _currentMouseCanvasPosition - this._viewModel.CanvasPosition;
        return true;
    }

    private void StopCanvasDragging() {
        GraphCanvasBg.ReleaseMouseCapture();
    }

    #endregion

    #region new connection

    const double PinConnectionDistance = 80;

    private Pin _activePin = null;
    private Pin _tempMousePin = null;
    private Pin _nearestPin = null;

    private void Node_OnPinClick(object sender, Pin e) {
        switch (_canvasState) {
            case CanvasState.Dragging:
            case CanvasState.StartCuttingConnections:
            case CanvasState.CuttingConnections:
            case CanvasState.Idle:
                CancelCuttingConnections();
                StopCanvasDragging();
                
                _activePin = e;
                _tempMousePin = new Pin {
                    DataType = e.DataType,
                    PinType = e.PinType.GetOpposite(),
                    CanvasPosition = _canvasMousePositionWithOffset
                };
                NewConnectionControl.Visibility = Visibility.Visible;

                NewConnectionControl.DataContext = new Connection(_activePin, _tempMousePin);
                // NewConnectionControl.CaptureMouse();
                
                _canvasState = CanvasState.NewConnection;
                return;
            
            case CanvasState.NewConnection:
                if (e.DataType == _activePin.DataType && e.PinType.GetOpposite() == _activePin.PinType) {
                    _viewModel.Connections.Add(new Connection(_activePin, e));
                    CancelNewConnection();
                    _canvasState = CanvasState.Idle;
                }
                return;
            
            default:
                throw new ArgumentOutOfRangeException();
        }
    }

    private void ContinueNewConnection() {
        Debug.Assert(_tempMousePin != null);

        _tempMousePin.CanvasPosition = _canvasMousePositionWithOffset;
        _nearestPin = _viewModel.Nodes
            .SelectMany(
                node => node.Events
                    .Concat(node.Inputs)
                    .Concat(node.Outputs)
                    .Concat([node.Trigger])
            )
            .Where(pin => pin.PinType == _tempMousePin.PinType && pin.DataType == _tempMousePin.DataType)
            .Where(pin => (pin.CanvasPosition - _tempMousePin.CanvasPosition).LengthSquared < PinConnectionDistance)
            .MinBy(pin => (pin.CanvasPosition - _tempMousePin.CanvasPosition).LengthSquared);
        if (_nearestPin != null) {
            _tempMousePin.CanvasPosition = _nearestPin.CanvasPosition;
        }
    }

    private void CancelNewConnection() {
        _tempMousePin = null;
        NewConnectionControl.Visibility = Visibility.Collapsed;
        NewConnectionControl.DataContext = null;
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

    private Point _previousCanvasMousePosition;
    private PathGeometry _cuttingPathGeometry;
    private PathFigure _cuttingPathFigure;

    private readonly List<ConnectionControl> _connectionControls = new();

    private bool StartCuttingConnections() {
        if (!GraphCanvasBg.CaptureMouse())
            return false;

        _previousCanvasMousePosition = _currentMouseCanvasPosition - _viewModel.CanvasPosition.ToVector();

        _cuttingPathFigure = new PathFigure(
            new Point(),
            [new LineSegment(_previousCanvasMousePosition, false)],
            false
        );

        CuttingPath.Data = _cuttingPathGeometry = new PathGeometry([_cuttingPathFigure]);
        CuttingPath.Visibility = Visibility.Visible;
        return true;
    }

    private bool ContinueCuttingConnections() {
        var delta = _canvasMousePositionWithOffset - _previousCanvasMousePosition;
        if (delta.Length < 4) return false;

        _cuttingPathFigure.Segments.Add(
            new LineSegment(_canvasMousePositionWithOffset, true)
        );
        _previousCanvasMousePosition = _canvasMousePositionWithOffset;

        foreach (var connectionControl in _connectionControls) {
            var points = GetIntersectionPoints(_cuttingPathGeometry, connectionControl.Geometry);
            if (points.Any()) {
                connectionControl.IsDeleted = true;
            }
        }

        return true;
    }

    private void ApplyCuttingConnections() {
        GraphCanvasBg.ReleaseMouseCapture();

        CuttingPath.Visibility = Visibility.Collapsed;
        CuttingPath.Data = null;
        _cuttingPathFigure = null;

        foreach (var connectionControl in _connectionControls.Where(ctrl => ctrl.IsDeleted)) {
            var connection = (Connection)connectionControl.DataContext;
            _viewModel.Connections.Remove(connection);
        }
    }

    private void CancelCuttingConnections() {
        GraphCanvasBg.ReleaseMouseCapture();

        CuttingPath.Visibility = Visibility.Collapsed;
        CuttingPath.Data = null;
        _cuttingPathFigure = null;

        _connectionControls.ForEach(ctrl => ctrl.IsDeleted = false);
    }

    private void ConnectionControl_OnLoaded(object sender, RoutedEventArgs e) {
        var connectionControl = (ConnectionControl)sender;
        _connectionControls.Add(connectionControl);
    }

    private void ConnectionControl_OnUnloaded(object sender, RoutedEventArgs e) {
        var connectionControl = (ConnectionControl)sender;
        _connectionControls.Remove(connectionControl);
    }

    #endregion

    private void Node_OnDeletePressed(object sender, EventArgs e) {
        var nodeControl = (NodeControl)sender;
        var node = nodeControl.NodeDataContext;
        _viewModel.Remove(node);
    }

    private void Node_OnSelected(object sender, EventArgs e) {
        var nodeControl = (NodeControl)sender;
        var node = nodeControl.NodeDataContext;
        _viewModel.SelectedNode = node;
    }
}