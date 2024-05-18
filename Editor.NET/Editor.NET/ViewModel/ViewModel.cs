using System;
using System.Collections.Generic;
using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Runtime.CompilerServices;
using System.Windows;

namespace Editor.NET.ViewModel;

public enum DataType {
    Number,
    String,
}

public class MainWindowViewModel : INotifyPropertyChanged {
    private Node? _selectoedNode;

    public MainWindowViewModel() {
    }

    public ObservableCollection<Node> Nodes { get; } = new();

    public ObservableCollection<Connection> Connections { get; } = new();

    public Node? SelectoedNode {
        get => _selectoedNode;
        set {
            if (Equals(value, _selectoedNode)) return;
            _selectoedNode = value;
            OnPropertyChanged();
        }
    }

    public event PropertyChangedEventHandler? PropertyChanged;

    protected virtual void OnPropertyChanged([CallerMemberName] string? propertyName = null) {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
    }

    protected bool SetField<T>(ref T field, T value, [CallerMemberName] string? propertyName = null) {
        if (EqualityComparer<T>.Default.Equals(field, value)) return false;
        field = value;
        OnPropertyChanged(propertyName);
        return true;
    }
}

public class Input : INotifyPropertyChanged {
    private DataType _type;
    private string _name = String.Empty;


    public String Name {
        get => _name;
        set {
            if (value == _name) return;
            _name = value;
            OnPropertyChanged();
        }
    }

    public DataType Type {
        get => _type;
        set {
            if (value == _type) return;
            _type = value;
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
    private bool _isEvent;

    public Point CanvasPosition {
        get => _canvasPosition;
        set {
            if (value.Equals(_canvasPosition)) return;
            _canvasPosition = value;
            OnPropertyChanged();
        }
    }

    public bool IsEvent {
        get => _isEvent;
        set {
            if (value == _isEvent) return;
            _isEvent = value;
            OnPropertyChanged();
        }
    }

    public event PropertyChangedEventHandler? PropertyChanged;

    protected virtual void OnPropertyChanged([CallerMemberName] string? propertyName = null) {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
    }

    protected bool SetField<T>(ref T field, T value, [CallerMemberName] string? propertyName = null) {
        if (EqualityComparer<T>.Default.Equals(field, value)) return false;
        field = value;
        OnPropertyChanged(propertyName);
        return true;
    }
}

public class Output : INotifyPropertyChanged {
    private string _name = String.Empty;
    private DataType _type;

    public String Name {
        get => _name;
        set {
            if (value == _name) return;
            _name = value;
            OnPropertyChanged();
        }
    }

    public DataType Type {
        get => _type;
        set {
            if (value == _type) return;
            _type = value;
            OnPropertyChanged();
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
    private bool _isEvent;

    public Point CanvasPosition {
        get => _canvasPosition;
        set {
            if (value.Equals(_canvasPosition)) return;
            _canvasPosition = value;
            OnPropertyChanged();
        }
    }

    public bool IsEvent {
        get => _isEvent;
        set {
            if (value == _isEvent) return;
            _isEvent = value;
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

    public event PropertyChangedEventHandler? PropertyChanged;

    protected virtual void OnPropertyChanged([CallerMemberName] string? propertyName = null) {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
    }

    protected bool SetField<T>(ref T field, T value, [CallerMemberName] string? propertyName = null) {
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
    private Input _trigger = new();


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

    public Input Trigger {
        get => _trigger;
        set {
            if (Equals(value, _trigger)) return;
            _trigger = value;
            OnPropertyChanged();
        }
    }

    public ObservableCollection<Input> Inputs { get; } = new();
    public ObservableCollection<Output> Outputs { get; } = new();
    public ObservableCollection<Output> Events { get; } = new();

    public event PropertyChangedEventHandler? PropertyChanged;

    protected virtual void OnPropertyChanged([CallerMemberName] string? propertyName = null) {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
    }

    protected bool SetField<T>(ref T field, T value, [CallerMemberName] string? propertyName = null) {
        if (EqualityComparer<T>.Default.Equals(field, value)) return false;
        field = value;
        OnPropertyChanged(propertyName);
        return true;
    }
}

public class Connection : INotifyPropertyChanged {
    public event PropertyChangedEventHandler? PropertyChanged;

    protected virtual void OnPropertyChanged([CallerMemberName] string? propertyName = null) {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
    }

    protected bool SetField<T>(ref T field, T value, [CallerMemberName] string? propertyName = null) {
        if (EqualityComparer<T>.Default.Equals(field, value)) return false;
        field = value;
        OnPropertyChanged(propertyName);
        return true;
    }

    public Output Output { get; init; }
    public Input Input { get; init; }

    public Connection(Output output, Input input) {
        Output = output;
        Input = input;
    }
}

public class DesignNode : Node {
    public DesignNode() {
        Name = "Node 1";
        Inputs.Add(new Input {
            Name = "Input 1",
            Type = DataType.Number,
        });
        Inputs.Add(new Input {
            Name = "Input 2",
            Type = DataType.String,
        });
        Outputs.Add(new Output {
            Name = "Result",
            Type = DataType.Number,
        });
        Outputs.Add(new Output {
            Name = "Output 2",
            Type = DataType.String,
        });
        Events.Add(new Output {
            IsEvent = true,
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