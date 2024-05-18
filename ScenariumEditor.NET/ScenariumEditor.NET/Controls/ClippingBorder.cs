using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;

namespace ScenariumEditor.NET.Controls;

public class ClippingBorder : Border {
    private readonly RectangleGeometry _clipRect = new RectangleGeometry();
    
    protected override void OnRender(DrawingContext dc) {
        _clipRect.RadiusX = _clipRect.RadiusY =
            Math.Max(0.0, this.CornerRadius.TopLeft - (this.BorderThickness.Left * 0.5));
        _clipRect.Rect = new Rect(this.RenderSize);
        this.Clip = _clipRect;
        
        base.OnRender(dc);
    }
}
