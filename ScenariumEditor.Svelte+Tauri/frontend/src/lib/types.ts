interface Pin {
    nodeId: string;
    type: 'input' | 'output';
    index: number;
}

interface NodeView {
    id: string;
    funcId: string;
    viewPos: {x: number; y: number};
    title: string;
    inputs: string[];
    outputs: string[];
}

interface ConnectionView {
    fromNodeId: string;
    fromIndex: number;
    toNodeId: string;
    toIndex: number;
}


interface GraphView {
    nodes: NodeView[];
    connections: ConnectionView[];
    viewScale: number;
    viewPos: {x: number; y: number};
    selectedNodeIds: Set<string>
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
