using System.Diagnostics;

namespace csso.Nodeshop.Editor;

public partial class MainPage : ContentPage {
    public MainPage() {
        InitializeComponent();
    }

    private void Button_OnClicked(object sender, EventArgs e) {
        Debug.WriteLine("Hello World!");
    }
}