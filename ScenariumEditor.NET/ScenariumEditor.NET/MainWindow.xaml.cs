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
using ScenariumEditor.NET.ViewModel;

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
        var uiElement = (UIElement)sender;
        uiElement.MouseMove += CanvasgBg_OnMouseMove_CanvasDragging;
        uiElement.MouseUp += CanvasgBg_OnButtonUp;

        uiElement.MouseMove += CanvasgBg_OnMouseMove_NewConnection;
        uiElement.PreviewMouseRightButtonDown += CanvasgBg_OnPreviewMouseRightButtonDown_NewConnection;
        uiElement.PreviewMouseLeftButtonDown += CanvasgBg_OnPreviewMouseLeftButtonDown_NewConnection;

        uiElement.MouseLeftButtonDown += CanvasgBg_OnMouseLeftButtonDown_CuttingConnections;
        uiElement.MouseMove += CanvasgBg_OnMouseMove_CuttingConnections;
        uiElement.MouseLeftButtonUp += CanvasgBg_OnMouseLeftButtonUp_CuttingConnections;
        uiElement.MouseRightButtonUp += CanvasgBg_OnMouseRightButtonUp_CuttingConnections;
    }

    #region canvas dragging

    private bool _isDragging = false;
    private Vector _canvasDragMousePosition;

    private void CanvasgBg_OnButtonDown(object sender, MouseButtonEventArgs e) {
        if (e.MiddleButton != MouseButtonState.Pressed) {
            return;
        }

        var canvas = (FrameworkElement)sender;
        if (canvas.CaptureMouse()) {
            _isDragging = true;
            _canvasDragMousePosition = e.GetPosition(canvas) - this._viewModel.CanvasPosition;
        }
    }

    private void CanvasgBg_OnButtonUp(object sender, MouseButtonEventArgs e) {
        var canvas = (FrameworkElement)sender;
        canvas.ReleaseMouseCapture();
        _isDragging = false;
    }

    private void CanvasgBg_OnMouseMove_CanvasDragging(object sender, MouseEventArgs e) {
        var canvas = (FrameworkElement)sender;
        var mousePosition = e.GetPosition(canvas);

        if (_isDragging) {
            var delta = mousePosition - _canvasDragMousePosition;
            this._viewModel.CanvasPosition = delta;
        }
    }

    #endregion

    private void CanvasgBg_OnMouseWheel(object sender, MouseWheelEventArgs e) {
        var canvas = (FrameworkElement)sender;
        var delta = e.Delta;
        this._viewModel.CanvasScale += delta / 2000.0;
    }

    #region new connection

    const double PinConnectionDistance = 80;

    private Point _canvasMousePosition;
    private bool _firstPinSelected = false;
    private Pin _activePin = null;
    private Pin _tempMousePin = null;
    private Pin _nearestPin = null;

    private void Node_OnPinActivated(object sender, Pin e) {
        if (!_firstPinSelected) {
            _firstPinSelected = true;

            _activePin = e;
            _tempMousePin = new Pin {
                DataType = e.DataType,
                PinType = e.PinType.GetOpposite(),
                CanvasPosition = _canvasMousePosition
            };
            NewConnectionControl.Visibility = Visibility.Visible;

            NewConnectionControl.DataContext = new Connection(_activePin, _tempMousePin);
            // NewConnectionControl.CaptureMouse();
        } else {
            _viewModel.Connections.Add(new Connection(_activePin, e));
            CancelNewConnection();
        }
    }

    private void CanvasgBg_OnMouseMove_NewConnection(object sender, MouseEventArgs e) {
        var canvas = (FrameworkElement)sender;
        _canvasMousePosition =
            e.GetPosition(canvas) - new Vector(_viewModel.CanvasPosition.X, _viewModel.CanvasPosition.Y);

        if (_tempMousePin != null) {
            _tempMousePin.CanvasPosition = _canvasMousePosition;
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
    }

    private void CancelNewConnection() {
        _firstPinSelected = false;
        _tempMousePin = null;
        NewConnectionControl.Visibility = Visibility.Collapsed;
        NewConnectionControl.DataContext = null;
    }

    private void CanvasgBg_OnPreviewMouseLeftButtonDown_NewConnection(object sender, MouseButtonEventArgs e) {
        if (_firstPinSelected && _nearestPin != null) {
            _viewModel.Connections.Add(new Connection(_activePin, _nearestPin));
            CancelNewConnection();
            e.Handled = true;
        }
    }

    private void CanvasgBg_OnPreviewMouseRightButtonDown_NewConnection(object sender, MouseButtonEventArgs e) {
        if (_firstPinSelected) {
            CancelNewConnection();
            e.Handled = true;
        }
    }

    #endregion

    #region cut connections

    public static Point[] GetIntersectionPoints(Geometry g1, Geometry g2) {
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

    private bool _cuttingConnections = false;
    private Point _previousMousePosition;
    private PathGeometry _cuttingPathGeometry;
    private PathFigure _cuttingPathFigure;

    private readonly List<ConnectionControl> _connectionControls = new();

    private void CanvasgBg_OnMouseLeftButtonDown_CuttingConnections(object sender, MouseButtonEventArgs e) {
        var uiElement = (UIElement)sender;
        if (uiElement.CaptureMouse()) {
            _cuttingConnections = true;
            _previousMousePosition = e.GetPosition(uiElement);

            _cuttingPathFigure = new PathFigure(
                new Point(),
                [new LineSegment(_previousMousePosition, false)],
                false);

            CuttingPath.Data = _cuttingPathGeometry = new PathGeometry([_cuttingPathFigure]);
            CuttingPath.Visibility = Visibility.Visible;
        }
    }

    private void CanvasgBg_OnMouseLeftButtonUp_CuttingConnections(object sender, MouseButtonEventArgs e) {
        if (_cuttingConnections) {
            var uiElement = (UIElement)sender;
            _cuttingConnections = false;
            uiElement.ReleaseMouseCapture();
            
            CuttingPath.Visibility = Visibility.Collapsed;
            CuttingPath.Data = null;
            _cuttingPathFigure = null;

            foreach (var connectionControl in _connectionControls.Where(ctrl=> ctrl.IsDeleted)) {
                var connection = (Connection)connectionControl.DataContext;
                _viewModel.Connections.Remove(connection);
            }
        }
    }
    
    private void CanvasgBg_OnMouseRightButtonUp_CuttingConnections(object sender, MouseButtonEventArgs e) {
        if (_cuttingConnections) {
            var uiElement = (UIElement)sender;
            _cuttingConnections = false;
            uiElement.ReleaseMouseCapture();

            CuttingPath.Visibility = Visibility.Collapsed;
            CuttingPath.Data = null;
            _cuttingPathFigure = null;
            
            _connectionControls.ForEach(ctrl=> ctrl.IsDeleted = false);
        }
    }


    private void CanvasgBg_OnMouseMove_CuttingConnections(object sender, MouseEventArgs e) {
        if (_cuttingConnections) {
            var uiElement = (UIElement)sender;
            var mousePosition = e.GetPosition(uiElement);
            var delta = mousePosition - _previousMousePosition;
            if (delta.LengthSquared > 10) {
                var controlPoint = new Point(
                    (_previousMousePosition.X + mousePosition.X) / 2,
                    (_previousMousePosition.Y + mousePosition.Y) / 2
                );

                _cuttingPathFigure.Segments.Add(
                    new QuadraticBezierSegment(controlPoint, mousePosition, true)
                );
                _previousMousePosition = mousePosition;


                foreach (var connectionControl in _connectionControls) {
                    var points = GetIntersectionPoints(_cuttingPathGeometry, connectionControl.Geometry);
                    if (points.Any()) {
                        connectionControl.IsDeleted = true;
                    }
                }
            }
        }
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
}