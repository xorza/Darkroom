using System;
using System.Collections.Generic;
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
using ViewModel;

namespace Editor.NET {
public partial class MainWindow : Window {
    private MainWindowViewModel _viewModel = new();

    public MainWindow() {
        InitializeComponent();

        this.DataContext = _viewModel;
    }

    private void AddDesignNodeButton_OnClick(object sender, RoutedEventArgs e) {
        _viewModel.Nodes.Add(new DesignNode());
    }
}
}