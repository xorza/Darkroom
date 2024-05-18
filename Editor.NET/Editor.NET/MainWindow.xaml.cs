using System;
using System.Collections.Generic;
using System.Collections.Specialized;
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
using Editor.NET.ViewModel;

namespace Editor.NET;

public partial class MainWindow : Window {
    private readonly MainWindowViewModel _viewModel = new DesignMainWindowViewModel();

    public MainWindow() {
        InitializeComponent();

        this.DataContext = _viewModel;

        _viewModel.Connections.CollectionChanged += (sender, args) => { RedrawConnections(); };
        RedrawConnections();
    }

    private void RedrawConnections() {
        ConnectionCanvas.Children.Clear();

        foreach (var connection in _viewModel.Connections) {
            var connectionView = new Connection {
                Thickness = 2,
            };

            connectionView.SetBinding(Connection.OutputPositionDependencyProperty,
                new Binding("Output.CanvasPosition") { Source = connection }
            );
            connectionView.SetBinding(Connection.InputPositionDependencyProperty,
                new Binding("Input.CanvasPosition") { Source = connection }
            );

            ConnectionCanvas.Children.Add(connectionView);
        }
    }

    private void AddDesignNodeButton_OnClick(object sender, RoutedEventArgs e) {
        _viewModel.Nodes.Add(new DesignNode());
    }

    private void MainWindow_OnLoaded(object sender, RoutedEventArgs e) {
    }
}