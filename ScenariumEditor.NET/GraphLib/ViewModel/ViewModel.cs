using System;
using System.Collections.Generic;
using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.CompilerServices;
using System.Windows;

namespace GraphLib.ViewModel;

public class MainWindowViewModel : INotifyPropertyChanged {
    public MainWindowViewModel() {
    }


    public ObservableCollection<Node> Nodes { get; } = new();

    public ObservableCollection<Connection> Connections { get; } = new();

    private Point _canvas_position;

    public Point CanvasPosition {
        get => _canvas_position;
        set {
            if (Equals(value, _canvas_position)) return;
            _canvas_position = value;
            OnPropertyChanged();
        }
    }

    private double _canvas_scale = 1.0;

    public double CanvasScale {
        get => _canvas_scale;
        set {
            if (Equals(value, _canvas_scale)) return;
            _canvas_scale = value;
            OnPropertyChanged();
        }
    }

    private Node _selected_node;

    public Node SelectedNode {
        get => _selected_node;
        set {
            if (ReferenceEquals(value, _selected_node)) return;
            if (_selected_node != null) {
                _selected_node.IsSelected = false;
            }

            _selected_node = value;

            if (_selected_node != null) {
                _selected_node.IsSelected = true;
            }

            OnPropertyChanged();
        }
    }

    public void Remove(Node node) {
        Nodes.Remove(node);
        var connections_to_remove = Connections
            .Where(c => {
                return node.Inputs.Contains(c.Input)
                       || node.Outputs.Contains(c.Output)
                       || node.Events.Contains(c.Output)
                       || ReferenceEquals(c.Input, node.Trigger);
            })
            .ToList();
        foreach (var connection in connections_to_remove) {
            Connections.Remove(connection);
        }
    }


    public event PropertyChangedEventHandler PropertyChanged;

    private void OnPropertyChanged([CallerMemberName] string property_name = null) {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(property_name));
    }

    private bool SetField<T>(ref T field, T value, [CallerMemberName] string property_name = null) {
        if (EqualityComparer<T>.Default.Equals(field, value)) return false;
        field = value;
        OnPropertyChanged(property_name);
        return true;
    }
}

public enum DataType {
    Number,
    String,
}

public enum PinType {
    Input,
    Output,
    Event,
    Trigger
}

public static class PinTypeExtensions {
    public static PinType GetOpposite(this PinType pin_type) {
        return pin_type switch {
            PinType.Input => PinType.Output,
            PinType.Output => PinType.Input,
            PinType.Event => PinType.Trigger,
            PinType.Trigger => PinType.Event,
            _ => throw new ArgumentOutOfRangeException(nameof(pin_type), pin_type, null)
        };
    }
}

public class Pin : INotifyPropertyChanged {
    private DataType _data_type;
    private string _name = String.Empty;


    public String Name {
        get => _name;
        set {
            if (value == _name) return;
            _name = value;
            OnPropertyChanged();
        }
    }

    public DataType DataType {
        get => _data_type;
        set {
            if (value == _data_type) return;
            _data_type = value;
            OnPropertyChanged();
        }
    }


    private Point _node_canvas_position;

    public Point NodeCanvasPosition {
        get => _node_canvas_position;
        set {
            if (value.Equals(_node_canvas_position)) return;
            _node_canvas_position = value;
            OnPropertyChanged();

            CanvasPosition = new Point(
                _node_canvas_position.X + _node_position.X,
                _node_canvas_position.Y + _node_position.Y
            );
        }
    }

    private Point _node_position;

    public Point NodePosition {
        get => _node_position;
        set {
            if (value.Equals(_node_position)) return;
            _node_position = value;
            OnPropertyChanged();

            CanvasPosition = new Point(
                _node_canvas_position.X + _node_position.X,
                _node_canvas_position.Y + _node_position.Y
            );
        }
    }


    private Point _canvas_position;

    public Point CanvasPosition {
        get => _canvas_position;
        set {
            if (value.Equals(_canvas_position)) return;
            _canvas_position = value;
            OnPropertyChanged();
        }
    }

    private PinType _pin_type;

    public PinType PinType {
        get => _pin_type;
        set {
            if (value.Equals(_pin_type)) return;
            _pin_type = value;
            OnPropertyChanged();
        }
    }


    public event PropertyChangedEventHandler PropertyChanged;

    protected virtual void OnPropertyChanged([CallerMemberName] string property_name = null) {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(property_name));
    }

    protected bool SetField<T>(ref T field, T value, [CallerMemberName] string property_name = null) {
        if (EqualityComparer<T>.Default.Equals(field, value)) return false;
        field = value;
        OnPropertyChanged(property_name);
        return true;
    }

    public static (Pin, Pin) Sort(Pin pin1, Pin pin2) {
        Debug.Assert(!ReferenceEquals(pin1, pin2));

        switch ((pin1.PinType, pin2.PinType)) {
            case (PinType.Input, PinType.Output):
                return (pin1, pin2);
            case (PinType.Output, PinType.Input):
                return (pin2, pin1);
            case (PinType.Event, PinType.Trigger):
                return (pin2, pin1);
            case (PinType.Trigger, PinType.Event):
                return (pin1, pin2);
            default:
                Debug.Assert(false);
                throw new InvalidOperationException("Invalid pin types");
        }
    }
}

public class Node : INotifyPropertyChanged {
    private string _name = string.Empty;
    private bool _is_selected = false;
    private bool _is_output = false;
    private bool _is_cached = false;
    private Point _canvas_position;

    private Pin _trigger = new Pin {
        Name = "Trigger",
        PinType = PinType.Trigger,
    };


    public String Name {
        get => _name;
        set {
            if (value == _name) return;
            _name = value;
            OnPropertyChanged();
        }
    }

    public bool IsSelected {
        get => _is_selected;
        internal set {
            if (value == _is_selected) return;
            _is_selected = value;
            OnPropertyChanged();
        }
    }

    public bool IsOutput {
        get => _is_output;
        set {
            if (value == _is_output) return;
            _is_output = value;
            OnPropertyChanged();
        }
    }

    public bool IsCached {
        get => _is_cached;
        set {
            if (value == _is_cached) return;
            _is_cached = value;
            OnPropertyChanged();
        }
    }

    public Point CanvasPosition {
        get => _canvas_position;
        set {
            if (value.Equals(_canvas_position)) return;
            _canvas_position = value;
            OnPropertyChanged();

            UpdatePinPositions();
        }
    }

    public void UpdatePinPositions() {
        var value = _canvas_position;

        _trigger.NodeCanvasPosition = value;

        foreach (var input in this.Inputs) {
            input.NodeCanvasPosition = value;
        }

        foreach (var @event in this.Events) {
            @event.NodeCanvasPosition = value;
        }

        foreach (var output in this.Outputs) {
            output.NodeCanvasPosition = value;
        }
    }

    public Pin Trigger {
        get => _trigger;
        set {
            if (Equals(value, _trigger)) return;
            _trigger = value;
            OnPropertyChanged();
        }
    }

    public ObservableCollection<Pin> Inputs { get; } = new();
    public ObservableCollection<Pin> Outputs { get; } = new();
    public ObservableCollection<Pin> Events { get; } = new();

    public event PropertyChangedEventHandler PropertyChanged;

    protected virtual void OnPropertyChanged([CallerMemberName] string property_name = null) {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(property_name));
    }

    protected bool SetField<T>(ref T field, T value, [CallerMemberName] string property_name = null) {
        if (EqualityComparer<T>.Default.Equals(field, value)) return false;
        field = value;
        OnPropertyChanged(property_name);
        return true;
    }
}

public class Connection : INotifyPropertyChanged {
    public event PropertyChangedEventHandler PropertyChanged;

    protected virtual void OnPropertyChanged([CallerMemberName] string property_name = null) {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(property_name));
    }

    protected bool SetField<T>(ref T field, T value, [CallerMemberName] string property_name = null) {
        if (EqualityComparer<T>.Default.Equals(field, value)) return false;
        field = value;
        OnPropertyChanged(property_name);
        return true;
    }

    public Pin Output { get; init; }
    public Pin Input { get; init; }

    public bool IsEvent { get; private init; }

    public Connection(Pin pin1, Pin pin2) {
        Debug.Assert(!ReferenceEquals(pin1, pin2));
        var (input, output) = Pin.Sort(pin1, pin2);
        Input = input;
        Output = output;
        IsEvent = input.PinType == PinType.Trigger;
    }
}

public class DesignNode : Node {
    public DesignNode() {
        Name = "Node 1";
        IsSelected = false;
        Inputs.Add(new Pin {
            Name = "Input 1",
            DataType = DataType.Number,
        });
        Inputs.Add(new Pin {
            Name = "Input 2",
            DataType = DataType.String,
        });
        Outputs.Add(new Pin {
            Name = "Result",
            PinType = PinType.Output,
            DataType = DataType.Number,
        });
        Outputs.Add(new Pin {
            Name = "Output 2",
            PinType = PinType.Output,
            DataType = DataType.String,
        });
        Events.Add(new Pin {
            PinType = PinType.Event,
            Name = "Event 1",
        });
    }
}

public class DesignMainWindowViewModel : MainWindowViewModel {
    public DesignMainWindowViewModel() {
        Nodes.Add(new DesignNode() {
            CanvasPosition = new Point(30.0f, 50.0f),
        });
        Nodes.Add(new DesignNode() {
            CanvasPosition = new Point(220.0f, 130.0f),
        });

        Connections.Add(new Connection(
            Nodes[0].Outputs[0],
            Nodes[1].Inputs[0]
        ));
    }
}