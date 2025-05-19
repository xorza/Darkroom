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
using CoreInterop;
using GraphLib.Controls;
using GraphLib.Utils;
using GraphLib.ViewModel;

namespace ScenariumEditor.NET;

public partial class MainWindow : Window {
    private readonly MainWindowViewModel _view_model = new DesignMainWindowViewModel();

    public MainWindow() {
        InitializeComponent();

        this.DataContext = _view_model;

        Loaded += MainWindow_OnLoaded;
        Unloaded += MainWindow_OnUnloaded;
    }

    private void MainWindow_OnLoaded(object sender, RoutedEventArgs e) {
    }

    private void MainWindow_OnUnloaded(object sender, RoutedEventArgs e) {
    }


    private void AddDesignNodeButton_OnClick(object sender, RoutedEventArgs e) {
        _view_model.Nodes.Add(new DesignNode());
    }
}