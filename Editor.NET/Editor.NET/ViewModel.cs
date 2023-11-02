using System;
using System.Collections.Generic;
using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices.JavaScript;

namespace ViewModel;

public enum DataType {
    Number,
    String,
}

public class MainWindowViewModel : INotifyPropertyChanged {
    private Node? _selectoedNode;

    public MainWindowViewModel() {
    }

    public ObservableCollection<Node> Nodes { get; } = new ObservableCollection<Node>();

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

public class Event {
    public String Name { get; set; } = String.Empty;
}

public class Node : INotifyPropertyChanged {
    private string _name = string.Empty;
    private bool _isSelected = false;
    private bool _isOutput = false;
    private bool _isCached = false;

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

    public ObservableCollection<Input> Inputs { get; } = new ObservableCollection<Input>();
    public ObservableCollection<Output> Outputs { get; } = new ObservableCollection<Output>();
    public ObservableCollection<Event> Events { get; } = new ObservableCollection<Event>();

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
        Events.Add(new Event {
            Name = "Event 1",
        });
    }
}

public class DesignMainWindowViewModel : MainWindowViewModel {
    public DesignMainWindowViewModel() {
        Nodes.Add(new DesignNode());
        Nodes.Add(new DesignNode());
    }
}