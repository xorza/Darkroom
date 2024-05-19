using System;
using System.Collections.Generic;
using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.CompilerServices;
using System.Windows;

namespace ScenariumEditor.NET.ViewModel;

public class MainWindowViewModel : INotifyPropertyChanged {
    public MainWindowViewModel() {
    }


    public ObservableCollection<Node> Nodes { get; } = new();

    public ObservableCollection<Connection> Connections { get; } = new();

    private Point _canvasPosition;

    public Point CanvasPosition {
        get => _canvasPosition;
        set {
            if (Equals(value, _canvasPosition)) return;
            _canvasPosition = value;
            OnPropertyChanged();
        }
    }

    private double _canvasScale = 1.0;

    public double CanvasScale {
        get => _canvasScale;
        set {
            if (Equals(value, _canvasScale)) return;
            _canvasScale = value;
            OnPropertyChanged();
        }
    }

    private Point _mouseCanvasPosition;

    public Point MouseCanvasPosition {
        get => _mouseCanvasPosition;
        set {
            if (Equals(value, _mouseCanvasPosition)) return;
            _mouseCanvasPosition = value;
            OnPropertyChanged();
        }
    }

    private Node _selectedNode;

    public Node SelectedNode {
        get => _selectedNode;
        set {
            if (ReferenceEquals(value, _selectedNode)) return;
            _selectedNode = value;
            OnPropertyChanged();
        }
    }

    private Pin _activePin;

    public Pin ActivePin {
        get => _activePin;
        set {
            if (ReferenceEquals(value, _activePin)) return;
            _activePin = value;
            OnPropertyChanged();
        }
    }

    public event PropertyChangedEventHandler PropertyChanged;

    private void OnPropertyChanged([CallerMemberName] string propertyName = null) {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
    }

    private bool SetField<T>(ref T field, T value, [CallerMemberName] string propertyName = null) {
        if (EqualityComparer<T>.Default.Equals(field, value)) return false;
        field = value;
        OnPropertyChanged(propertyName);
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

public class Pin : INotifyPropertyChanged {
    private DataType _dataType;
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
        get => _dataType;
        set {
            if (value == _dataType) return;
            _dataType = value;
            OnPropertyChanged();
        }
    }


    private Point _nodeCanvasPosition;

    public Point NodeCanvasPosition {
        get => _nodeCanvasPosition;
        set {
            if (value.Equals(_nodeCanvasPosition)) return;
            _nodeCanvasPosition = value;
            OnPropertyChanged();

            CanvasPosition = new Point(
                _nodeCanvasPosition.X + _nodePosition.X,
                _nodeCanvasPosition.Y + _nodePosition.Y
            );
        }
    }

    private Point _nodePosition;

    public Point NodePosition {
        get => _nodePosition;
        set {
            if (value.Equals(_nodePosition)) return;
            _nodePosition = value;
            OnPropertyChanged();

            CanvasPosition = new Point(
                _nodeCanvasPosition.X + _nodePosition.X,
                _nodeCanvasPosition.Y + _nodePosition.Y
            );
        }
    }


    private Point _canvasPosition;

    public Point CanvasPosition {
        get => _canvasPosition;
        set {
            if (value.Equals(_canvasPosition)) return;
            _canvasPosition = value;
            OnPropertyChanged();
        }
    }

    private PinType _pinType;

    public PinType PinType {
        get => _pinType;
        set {
            if (value.Equals(_pinType)) return;
            _pinType = value;
            OnPropertyChanged();
        }
    }


    public event PropertyChangedEventHandler PropertyChanged;

    protected virtual void OnPropertyChanged([CallerMemberName] string propertyName = null) {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
    }

    protected bool SetField<T>(ref T field, T value, [CallerMemberName] string propertyName = null) {
        if (EqualityComparer<T>.Default.Equals(field, value)) return false;
        field = value;
        OnPropertyChanged(propertyName);
        return true;
    }
}

public class Node : INotifyPropertyChanged {
    private string _name = string.Empty;
    private bool _isSelected = false;
    private bool _isOutput = false;
    private bool _isCached = false;
    private Point _canvasPosition;

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
        get => _isSelected;
        set {
            if (value == _isSelected) return;
            _isSelected = value;
            OnPropertyChanged();
        }
    }

    public bool IsOutput {
        get => _isOutput;
        set {
            if (value == _isOutput) return;
            _isOutput = value;
            OnPropertyChanged();
        }
    }

    public bool IsCached {
        get => _isCached;
        set {
            if (value == _isCached) return;
            _isCached = value;
            OnPropertyChanged();
        }
    }

    public Point CanvasPosition {
        get => _canvasPosition;
        set {
            if (value.Equals(_canvasPosition)) return;
            _canvasPosition = value;
            OnPropertyChanged();

            UpdatePinPositions();
        }
    }

    public void UpdatePinPositions() {
        var value = _canvasPosition;

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

    protected virtual void OnPropertyChanged([CallerMemberName] string propertyName = null) {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
    }

    protected bool SetField<T>(ref T field, T value, [CallerMemberName] string propertyName = null) {
        if (EqualityComparer<T>.Default.Equals(field, value)) return false;
        field = value;
        OnPropertyChanged(propertyName);
        return true;
    }
}

public class Connection : INotifyPropertyChanged {
    public event PropertyChangedEventHandler PropertyChanged;

    protected virtual void OnPropertyChanged([CallerMemberName] string propertyName = null) {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
    }

    protected bool SetField<T>(ref T field, T value, [CallerMemberName] string propertyName = null) {
        if (EqualityComparer<T>.Default.Equals(field, value)) return false;
        field = value;
        OnPropertyChanged(propertyName);
        return true;
    }

    public Pin Output { get; init; }
    public Pin Input { get; init; }

    public Connection(Pin pin1, Pin pin2) {
        Debug.Assert(!ReferenceEquals(pin1, pin2));

        switch ((pin1.PinType, pin2.PinType)) {
            case (PinType.Input, PinType.Output):
                Output = pin2;
                Input = pin1;
                break;
            case (PinType.Output, PinType.Input):
                Output = pin1;
                Input = pin2;
                break;
            case (PinType.Event, PinType.Trigger):
                Output = pin1;
                Input = pin2;
                break;
            case (PinType.Trigger, PinType.Event):
                Input = pin1;
                Output = pin2;
                break;
            default:
                Debug.Assert(false);
                break;
        }
    }
}

public class DesignNode : Node {
    public DesignNode() {
        Name = "Node 1";
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