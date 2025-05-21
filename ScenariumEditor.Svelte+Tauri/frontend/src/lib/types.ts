interface Pin {
    nodeId: number;
    type: 'input' | 'output';
    index: number;
}

interface NodeView {
    id: string;
    funcId: string;
    x: number;
    y: number;
    title: string;
    inputs: string[];
    outputs: string[];
}

interface ConnectionView {
    fromNodeId: number;
    fromIndex: number;
    toNodeId: number;
    toIndex: number;
}


interface GraphView {
    nodes: NodeView[];
    connections: ConnectionView[];
    viewScale: number;
    viewX: number;
    viewY: number;
    selectedNodeIds: Set<number>
}

interface FuncView {
    id: string;
    title: string;
    description: string;
}

interface FuncLibraryView {
    funcs: FuncView[];
}

export type {
    NodeView,
    ConnectionView,
    GraphView,
    Pin,
    FuncView,
    FuncLibraryView
}
